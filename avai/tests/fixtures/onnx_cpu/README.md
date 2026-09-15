# AVAI ONNX CPU fixture

This project-created fixture is licensed under Apache-2.0 (see `LICENSE`). It contains:

- `model.onnx`: fixed `f32 [1,3,1,1]` input `input`, Add `[1,2,3]`, fixed output `output`;
- `input.png`: one RGB pixel `[10,20,30]`;
- `expected.json`: deterministic `tensor_json_v1` output `[11,22,33]`.
- `termination-stress.onnx`: 128 sequential `Sin` operations over `f32 [1,3,512,512]`, used only to observe cooperative termination within the acceptance budget.

It was generated with `generate_fixture.py`, ONNX `onnx.proto` from tag `v1.17.0`, protobuf `3.20.3`, and `protoc 3.6.1`. The generator uses no model or runtime download.
