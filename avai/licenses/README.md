# ONNX CPU runtime license inventory

The AVAI Linux x86_64 GNU release layout installs these files under
`<release>/licenses/` alongside the separately packaged native runtime:

- `onnxruntime/LICENSE` and `onnxruntime/ThirdPartyNotices.txt` are from the
  Microsoft ONNX Runtime 1.28.0 CPU x64 release archive
  `onnxruntime-linux-x64-1.28.0.tgz`.
- `ort/LICENSE-MIT` and `ort/LICENSE-APACHE` are from the Rust `ort` crate
  version `2.0.0-rc.13`.

The verified native archive provenance is the GitHub release `microsoft/onnxruntime`
tag `v1.28.0`, asset `onnxruntime-linux-x64-1.28.0.tgz`, with SHA-256
`a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407`.
Its `GIT_COMMIT_ID` is `da9b5e364c465de65c49d91e696cd6485270757f`.

These metadata files do not authorize runtime downloads. Deployment must place the
pre-verified CPU library at
`<release>/lib/onnxruntime/1.28.0/libonnxruntime.so.1.28.0` before AVAI starts.
