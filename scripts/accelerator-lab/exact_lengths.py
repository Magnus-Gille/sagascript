"""Make nonnegative length truncation explicit before accelerator float-to-int casts.

For valid feature lengths >=1, every subsampler Div result is nonnegative.
ONNX Cast truncates; Floor then Cast is equivalent and avoids backend rounding
of fractional lengths. The hardware regression is qnn_length_probe.py.
"""
import argparse
from pathlib import Path
import onnx


def patch(source,target):
    source,target=Path(source).resolve(),Path(target).resolve()
    if target.exists(): raise FileExistsError(target)
    if source.parent!=target.parent: raise ValueError('Preserve external-data base directory')
    model=onnx.load(source,load_external_data=False)
    selected={'/pre_encode/conv/Cast_4','/pre_encode/conv/Cast_9','/pre_encode/conv/Cast_14'}
    producers={o:n for n in model.graph.node for o in n.output}
    nodes=[];found=set()
    for n in model.graph.node:
        if n.name in selected:
            if n.op_type!='Cast' or not any(a.name=='to' and a.i==onnx.TensorProto.INT64 for a in n.attribute) or producers[n.input[0]].op_type!='Div':
                raise ValueError('Unexpected subsampling length graph')
            name=n.name+'_floor'
            nodes.append(onnx.helper.make_node('Floor',[n.input[0]],[name],name=name))
            n.input[0]=name;found.add(n.name)
        nodes.append(n)
    if found!=selected: raise ValueError('Expected exactly three length casts')
    del model.graph.node[:];model.graph.node.extend(nodes)
    onnx.save(model,target);onnx.checker.check_model(str(target))


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('source',type=Path);p.add_argument('target',type=Path)
    a=p.parse_args();patch(a.source,a.target)
