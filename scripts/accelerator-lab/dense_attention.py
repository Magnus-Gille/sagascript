"""Port the Mac DenseLocalAttention algebra to the pinned Pianissimo ONNX graph.

Unlike the fixed-full-window Mac adapter, retain the ONNX source's padding
mask. No retraining, parameter change or dependency on Torch/CoreML is needed.
The original conversion provenance remains scripts/pianissimo-coreml/components.py.
"""
import argparse
import json
from pathlib import Path
import numpy as np
import onnx
from onnx import helper as h, numpy_helper as nh, TensorProto as T


def attention_nodes(prefix, frames, heads=8, width=128, context=256, inputs=None):
    if min(frames, heads, width, context) <= 0:
        raise ValueError('Positive static attention dimensions required')
    names = dict(zip(('q','k','v','pos','u','w','pad'), ('q','k','v','pos','u','w','pad')))
    names.update(inputs or {})
    nodes, tensors = [], []
    def constant(name, value):
        out = prefix + '/' + name
        tensors.append(nh.from_array(np.asarray(value), out))
        return out
    def op(kind, values, name, **attrs):
        out = prefix + '/' + name
        nodes.append(h.make_node(kind, values, [out], name=out, **attrs))
        return out
    qi, ki = np.arange(frames)[:,None], np.arange(frames)[None,:]
    indices = np.broadcast_to(np.clip(ki-qi+context,0,2*context), (1,heads,frames,frames)).astype('int64').copy()
    relative = constant('relative', indices)
    band = constant('outside_band', (np.abs(ki-qi)>context)[None,None,:,:])
    scalar = constant('scale', np.array(np.sqrt(width), dtype='float32'))
    neg = constant('negative', np.array(-10000, dtype='float32'))
    zero = constant('zero', np.array(0, dtype='float32'))
    kt = op('Transpose', [names['k']], 'kt', perm=[0,1,3,2])
    qu = op('Add',[names['q'],names['u']],'qu')
    qw = op('Add',[names['q'],names['w']],'qw')
    ac = op('MatMul',[qu,kt],'ac')
    bd = op('MatMul',[qw,names['pos']],'bd')
    bd = op('GatherElements',[bd,relative],'bd_relative',axis=3)
    scores = op('Div',[op('Add',[ac,bd],'scores_sum'),scalar],'scores')
    key_mask = op('Reshape',[names['pad'],constant('key_shape',np.array([1,1,1,frames],dtype='int64'))],'key_mask')
    mask = op('Or',[key_mask,band],'mask')
    masked = op('Where',[mask,neg,scores],'masked')
    probabilities = op('Softmax',[masked],'probabilities',axis=3)
    query_mask = op('Reshape',[names['pad'],constant('query_shape',np.array([1,1,frames,1],dtype='int64'))],'query_mask')
    probabilities = op('Where',[query_mask,zero,probabilities],'valid_probabilities')
    output = op('MatMul',[probabilities,names['v']],'weighted_values')
    return nodes,tensors,output


def transform(model, frames):
    encoded = (frames+7)//8
    existing = {n.name:n for n in model.graph.node}
    replacements = {}
    added = []
    for layer in range(24):
        prefix = f'/layers.{layer}/self_attn'
        def source(suffix):
            return existing[prefix+'/'+suffix]
        projections = [source('linear_'+s+'/MatMul') for s in ('q','k','v','pos')]
        outgoing = source('linear_out/MatMul')
        if any(n.op_type!='MatMul' for n in projections+[outgoing]):
            raise ValueError('Unsupported source attention layout')
        nodes = list(projections)
        def const(suffix, values):
            name = prefix+'/dense/'+suffix
            added.append(nh.from_array(np.array(values,dtype='int64'),name))
            return name
        def op(kind, inputs, suffix, **attrs):
            name = prefix+'/dense/'+suffix
            nodes.append(h.make_node(kind,inputs,[name],name=name,**attrs))
            return name
        qkv = [op('Transpose',[op('Reshape',[n.output[0],const('shape_'+s,[1,encoded,8,128])],'reshape_'+s)],s,perm=[0,2,1,3]) for s,n in zip(('q','k','v'),projections)]
        pos = op('Transpose',[op('Reshape',[projections[3].output[0],const('pos_shape',[1,513,8,128])],'pos_reshape')],'pos',perm=[0,2,3,1])
        mapping = dict(zip(('q','k','v'),qkv))
        mapping.update(pos=pos,u=source('Add').input[1],w=source('Add_1').input[1],pad='/Not_output_0')
        attention, tensors, result = attention_nodes(prefix+'/dense/attention',encoded,inputs=mapping)
        nodes.extend(attention); added.extend(tensors)
        result = op('Transpose',[result],'to_time',perm=[0,2,1,3])
        result = op('Reshape',[result,const('output_shape',[1,encoded,1024])],'output')
        nodes.append(h.make_node('MatMul',[result,outgoing.input[1]],list(outgoing.output),name=outgoing.name))
        replacements[outgoing.name] = nodes
    before = len(model.graph.node)
    all_nodes=[]
    for n in model.graph.node:
        if n.name in replacements:
            all_nodes.extend(replacements[n.name])
        elif not any(n.name.startswith(f'/layers.{i}/self_attn/') for i in range(24)):
            all_nodes.append(n)
    # Prune removed attention helpers, while retaining any shared parameter cast
    # or constant still reachable from an output. Source graph is topological.
    needed={x.name for x in model.graph.output}
    live=[]
    for n in reversed(all_nodes):
        if any(o in needed for o in n.output):
            live.append(n); needed.update(n.input)
    live.reverse()
    del model.graph.node[:]; model.graph.node.extend(live)
    weights=[x for x in model.graph.initializer if x.name in needed]+[x for x in added if x.name in needed]
    del model.graph.initializer[:]; model.graph.initializer.extend(weights)
    # Old symbolic shape facts cannot describe the new intermediate tensors.
    del model.graph.value_info[:]
    for inp in model.graph.input:
        sizes = [1,128,frames] if inp.name=='audio_signal' else [1]
        for dim,size in zip(inp.type.tensor_type.shape.dim,sizes):
            dim.ClearField('dim_param'); dim.dim_value=size
    return {'layers':24,'nodes_before':before,'nodes_after':len(live),'mel_frames':frames,'encoded_frames':encoded}


def main(a):
    if a.target.exists() or a.target.with_suffix('.weights').exists():
        raise FileExistsError('Refusing to overwrite a prior model')
    model=onnx.load(a.source)
    result=transform(model,a.frames)
    onnx.checker.check_model(model)
    onnx.save_model(model,a.target,save_as_external_data=True,all_tensors_to_one_file=True,
                    location=a.target.with_suffix('.weights').name,size_threshold=1024)
    result.update(source=str(a.source),output=str(a.target),padding_mask='preserved',basis='Mac DenseLocalAttention')
    a.target.with_suffix('.conversion.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps(result),flush=True)


if __name__=='__main__':
    p=argparse.ArgumentParser()
    p.add_argument('source',type=Path);p.add_argument('target',type=Path)
    p.add_argument('--frames',type=int,default=2048)
    main(p.parse_args())
