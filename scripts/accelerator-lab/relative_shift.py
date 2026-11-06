"""QNN-friendly relative attention: constant reshape/slice instead of GatherElements.

For each head and query i, desired bd[i, context+j-i] has flat index
i*(2*context)+context+j. Slice at context, reshape rows at stride2*context,
then retain the first frames columns. Out-of-band entries are masked later.
"""
import argparse
from pathlib import Path
import numpy as np
import onnx
from onnx import helper as h,numpy_helper as nh


def shift_nodes(data,output,prefix,frames,heads=8,context=256):
    if not 0<frames<=2*context: raise ValueError('Dense shift supports 1..2*context frames')
    nodes=[];initializers=[]
    def const(name,values,dtype='int64'):
        name=prefix+'/'+name;initializers.append(nh.from_array(np.array(values,dtype=dtype),name));return name
    def op(kind,inputs,name,**attrs):
        name=prefix+'/'+name;nodes.append(h.make_node(kind,inputs,[name],name=name,**attrs));return name
    flat=op('Reshape',[data,const('flat_shape',[1,heads,frames*(2*context+1)])],'flat')
    if frames<context:
        flat=op('Pad',[flat,const('pads',[0,0,0,0,0,context-frames]),const('zero',0,'float32')],'padded')
    sliced=op('Slice',[flat,const('start',[context]),const('end',[context+frames*2*context]),const('axis',[2])],'shifted')
    shaped=op('Reshape',[sliced,const('matrix_shape',[1,heads,frames,2*context])],'matrix')
    nodes.append(h.make_node('Slice',[shaped,const('column_start',[0]),const('column_end',[frames]),const('column_axis',[3])],[output],name=prefix+'/columns'))
    return nodes,initializers


def patch(source,target,frames=None):
    source,target=Path(source).resolve(),Path(target).resolve()
    if source.parent!=target.parent: raise ValueError('Preserve external weights base')
    if target.exists(): raise FileExistsError(target)
    model=onnx.load(source,load_external_data=False)
    audio = next((i for i in model.graph.input if i.name == 'audio_signal'), None)
    if audio is None or len(audio.type.tensor_type.shape.dim) != 3:
        raise ValueError('Expected a fixed audio_signal input')
    mel_frames = audio.type.tensor_type.shape.dim[2].dim_value
    if mel_frames <= 0:
        raise ValueError('Expected fixed positive mel frames')
    encoded_frames = (mel_frames + 7) // 8
    if frames is not None and frames != encoded_frames:
        raise ValueError(f'Expected {encoded_frames} encoded frames from source, got {frames}')
    frames = encoded_frames
    nodes=[];added=[];count=0;old_indices=set()
    for n in model.graph.node:
        if n.op_type=='GatherElements' and n.name.endswith('/dense/attention/bd_relative'):
            ns,ts=shift_nodes(n.input[0],n.output[0],n.name+'_shift',frames)
            nodes.extend(ns);added.extend(ts);old_indices.add(n.input[1]);count+=1
        else: nodes.append(n)
    if count!=24: raise ValueError(f'Expected24attention gathers, got{count}')
    del model.graph.node[:];model.graph.node.extend(nodes)
    weights=[i for i in model.graph.initializer if i.name not in old_indices]+added
    del model.graph.initializer[:];model.graph.initializer.extend(weights)
    onnx.save(model,target);onnx.checker.check_model(str(target))


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('source',type=Path);p.add_argument('target',type=Path);p.add_argument('--frames',type=int)
    a=p.parse_args();patch(a.source,a.target,a.frames)
