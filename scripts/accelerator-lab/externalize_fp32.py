"""Promote half-stored FP32 weights one tensor at a time, without full model copies.

Pianissimo's FP16 ONNX export stores constants as half then Casts them to FLOAT.
QDQ needs those FLOAT constants exposed directly. Read graph-only, stream each
initializer into a new external file, and migrate the small graph to opset21.
"""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import numpy as np
import onnx
from onnx import helper as h, numpy_helper as nh, TensorProto as T


def convert(source, target):
    source,target=Path(source).resolve(),Path(target).resolve()
    weights=target.with_suffix('.weights')
    if target.exists() or weights.exists():
        raise FileExistsError('Refusing to overwrite model or external weights')
    if source.parent!=target.parent:
        raise ValueError('Keep output beside source to retain unchanged external-data references')
    model=onnx.load(source,load_external_data=False)
    consumers=defaultdict(list)
    for node in model.graph.node:
        for inp in node.input:
            consumers[inp].append(node)
    keep=[]; removed=set(); aliases=[]; count=0; max_tensor=0
    with weights.open('xb') as stream:
        for tensor in model.graph.initializer:
            uses=consumers[tensor.name]
            if tensor.data_type!=T.FLOAT16 or not uses or not all(n.op_type=='Cast' and any(a.name=='to' and a.i==T.FLOAT for a in n.attribute) for n in uses):
                keep.append(tensor);continue
            # to_array loads this initializer only; avoid retaining its raw_data
            # after it has been written. Peak scales with largest weight, not all.
            array=nh.to_array(tensor,base_dir=str(source.parent)).astype('<f4')
            offset=stream.tell(); stream.write(memoryview(array).cast('B'))
            length=array.nbytes; max_tensor=max(max_tensor,length)
            name=uses[0].output[0]
            full=T();full.name=name;full.data_type=T.FLOAT;full.dims.extend(tensor.dims)
            full.data_location=T.EXTERNAL
            for key,value in [('location',weights.name),('offset',str(offset)),('length',str(length))]:
                entry=full.external_data.add();entry.key=key;entry.value=value
            keep.append(full);count+=1
            for node in uses:
                removed.add(node.name)
                if node.output[0]!=name:
                    aliases.append(h.make_node('Identity',[name],[node.output[0]],name=node.name+'_alias'))
            tensor.ClearField('raw_data')
            del array
    nodes=aliases+[n for n in model.graph.node if n.name not in removed]
    del model.graph.node[:];model.graph.node.extend(nodes)
    del model.graph.initializer[:];model.graph.initializer.extend(keep)
    model=onnx.version_converter.convert_version(model,21)
    onnx.save(model,target)
    onnx.checker.check_model(str(target))
    result={'source':str(source),'target':str(target),'source_graph_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),
            'promoted_initializers':count,'largest_promoted_tensor_bytes':max_tensor,'external_weight_bytes':weights.stat().st_size,
            'opset':21,'semantics':'exact FP16-to-FP32 cast values preserved'}
    target.with_suffix('.conversion.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    return result


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('source',type=Path);p.add_argument('target',type=Path)
    a=p.parse_args();print(json.dumps(convert(a.source,a.target)),flush=True)
