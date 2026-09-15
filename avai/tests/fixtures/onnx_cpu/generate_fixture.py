"""Regenerate the project-created WP-03G ONNX/PNG fixture.

Requires Python 3, protobuf 3.20.x, protoc, and ONNX's onnx.proto from tag v1.17.0
compiled as onnx_pb2.py on PYTHONPATH. No network access is used by this script.
"""

import binascii
import struct
import zlib

import onnx_pb2 as onnx


def value_info(name):
    value = onnx.ValueInfoProto(name=name)
    tensor = value.type.tensor_type
    tensor.elem_type = onnx.TensorProto.FLOAT
    for size in (1, 3, 1, 1):
        tensor.shape.dim.add().dim_value = size
    return value


def png_chunk(kind, data):
    return (
        struct.pack(">I", len(data))
        + kind
        + data
        + struct.pack(">I", binascii.crc32(kind + data) & 0xFFFFFFFF)
    )


graph = onnx.GraphProto(name="avai-add-rgb-v1")
graph.input.append(value_info("input"))
graph.output.append(value_info("output"))
constant = graph.initializer.add()
constant.name = "constant"
constant.data_type = onnx.TensorProto.FLOAT
constant.dims.extend((1, 3, 1, 1))
constant.raw_data = struct.pack("<fff", 1.0, 2.0, 3.0)
graph.node.add(name="add-rgb", op_type="Add", input=["input", "constant"], output=["output"])

model = onnx.ModelProto(ir_version=10, producer_name="epimore-gmv-wp03g", graph=graph)
model.opset_import.add(domain="", version=13)
with open("model.onnx", "wb") as output:
    output.write(model.SerializeToString())

stress_graph = onnx.GraphProto(name="avai-termination-stress-v2")
stress_graph.input.append(value_info("input"))
stress_graph.output.append(value_info("output"))
stress_shape = stress_graph.initializer.add()
stress_shape.name = "stress-shape"
stress_shape.data_type = onnx.TensorProto.INT64
stress_shape.dims.extend((4,))
stress_shape.raw_data = struct.pack("<qqqq", 1, 3, 1024, 1024)
stress_graph.node.add(
    name="expand-input",
    op_type="Expand",
    input=["input", "stress-shape"],
    output=["expanded"],
)
previous = "expanded"
for index in range(256):
    current = f"sin-{index}"
    stress_graph.node.add(
        name=f"sin-{index}", op_type="Sin", input=[previous], output=[current]
    )
    previous = current
reduction = stress_graph.node.add(
    name="reduce-output", op_type="ReduceMean", input=[previous], output=["output"]
)
reduction.attribute.add(name="axes", ints=[2, 3], type=onnx.AttributeProto.INTS)
reduction.attribute.add(name="keepdims", i=1, type=onnx.AttributeProto.INT)
stress_model = onnx.ModelProto(
    ir_version=10, producer_name="epimore-gmv-wp03g", graph=stress_graph
)
stress_model.opset_import.add(domain="", version=13)
with open("termination-stress.onnx", "wb") as output:
    output.write(stress_model.SerializeToString())

scanline = b"\x00\x0a\x14\x1e"
png = b"\x89PNG\r\n\x1a\n"
png += png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
png += png_chunk(b"IDAT", zlib.compress(scanline, 9))
png += png_chunk(b"IEND", b"")
with open("input.png", "wb") as output:
    output.write(png)
