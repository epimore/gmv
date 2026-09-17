# GMV Model Package contract

`gmv-model-package` is the portable contract shared by AVAI and GMVC. It owns the Model Package manifest DTOs, canonical signing payload, Ed25519 verification primitive, target-independent structural validation, confined relative-path rules, and the streaming Model Bundle v1 reader.

Model Bundle v1 is an uncompressed tar stream. `manifest.yaml` must be the first regular-file entry. The reader never extracts the archive: it rejects non-UTF-8, absolute, traversal, duplicate, undeclared, link, device, and FIFO entries while streaming every declared file through its size and SHA-256 checks. The manifest, file count, unpacked byte count, path length/depth, and JSON result schema are bounded.

This crate deliberately does not own:

- GMVC registry or release state;
- signing-key or license policy configuration;
- target compatibility and variant selection;
- AVAI installation, self-test execution, health, activation, or rollback;
- database, transport, Desired, MQTT, Agent, or delivery side effects.

Callers provide trusted public keys and apply their own policy after cryptographic and structural verification. A signature declaration in YAML is not evidence until `verify_model_package_signature` or `verify_model_bundle` succeeds with a caller-supplied trusted key.
