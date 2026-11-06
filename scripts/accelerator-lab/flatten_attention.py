"""Flatten ONLY the three rank-5 attention matmuls per layer to rank 3.

Explicitly broadcast batch axes before flattening, preserving ONNX MatMul
semantics for all three attention products. Original models stay untouched.
Validate CPU outputs/text against the original before using on an accelerator.
"""
import argparse
import re
import onnx
from onnx import helper, TensorProto

p = argparse.ArgumentParser()
p.add_argument('source')
p.add_argument('target')
a = p.parse_args()
m = onnx.load(a.source)
nodes, count = [], 0
for n in m.graph.node:
    if n.op_type != 'MatMul' or not re.fullmatch(r'/layers\.\d+/self_attn/MatMul(?:_[12])?', n.name):
        nodes.append(n)
        continue
    prefix = 'labflat_' + str(count) + '_'
    count += 1
    def const(label, values):
        name = prefix + label
        m.graph.initializer.append(helper.make_tensor(name, TensorProto.INT64, [len(values)], values))
        return name
    def op(kind, inputs, label, **attrs):
        output = prefix + label
        nodes.append(helper.make_node(kind, inputs, [output], name=output, **attrs))
        return output
    idx_batch, idx_matrix = const('batchindices',[0,1,2]), const('matrixindices',[3,4])
    idx_m, idx_n, minus1 = const('mindex',[3]), const('nindex',[4]), const('minus1',[-1])
    sa, sb = op('Shape',[n.input[0]],'shapea'), op('Shape',[n.input[1]],'shapeb')
    ba, bb = op('Gather',[sa,idx_batch],'batcha',axis=0), op('Gather',[sb,idx_batch],'batchb',axis=0)
    batch = op('Max',[ba,bb],'batch')
    ma, mb = op('Gather',[sa,idx_matrix],'matrixa',axis=0), op('Gather',[sb,idx_matrix],'matrixb',axis=0)
    ea_shape = op('Concat',[batch,ma],'expandashape',axis=0)
    eb_shape = op('Concat',[batch,mb],'expandbshape',axis=0)
    ea, eb = op('Expand',[n.input[0],ea_shape],'expanda'), op('Expand',[n.input[1],eb_shape],'expandb')
    fa_shape, fb_shape = op('Concat',[minus1,ma],'flatashape',axis=0), op('Concat',[minus1,mb],'flatbshape',axis=0)
    fa, fb = op('Reshape',[ea,fa_shape],'flata'), op('Reshape',[eb,fb_shape],'flatb')
    product = op('MatMul',[fa,fb],'matmul')
    dim_m, dim_n = op('Gather',[sa,idx_m],'m',axis=0), op('Gather',[sb,idx_n],'n',axis=0)
    shape = op('Concat',[batch,dim_m,dim_n],'outshape',axis=0)
    nodes.append(helper.make_node('Reshape',[product,shape],list(n.output),name=n.name+'_restore_rank'))
assert count == 72, f'Expected 3 matmuls x 24 layers, got {count}'
del m.graph.node[:]
m.graph.node.extend(nodes)
onnx.checker.check_model(m)
onnx.save(m,a.target)
print(f'Rewrote {count} rank-5 attention matrix products; saved {a.target}')
