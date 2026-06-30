use std::io;
use std::path::PathBuf;

fn main() -> io::Result<()> {
    let proto_root = PathBuf::from("proto");
    let protos = [
        proto_root.join("common/v1/types.proto"),
        proto_root.join("guard/v1/node_control.proto"),
        proto_root.join("guard/v1/control.proto"),
        proto_root.join("session/v1/control.proto"),
        proto_root.join("stream/v1/control.proto"),
        proto_root.join("avai/v1/control.proto"),
    ];

    println!("cargo:rerun-if-changed=build.rs");
    for proto in &protos {
        println!("cargo:rerun-if-changed={}", proto.display());
    }
    for entry in proto_root.read_dir()? {
        let entry = entry?;
        if entry.path().is_dir() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }

    let protoc = protoc_bin_vendored::protoc_bin_path().map_err(vendored_error)?;
    let vendored_include = protoc_bin_vendored::include_path().map_err(vendored_error)?;
    let descriptor_path = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"))
        .join("gmv_protocol_descriptor.bin");

    let mut prost = tonic_prost_build::Config::new();
    prost.protoc_executable(protoc);

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .file_descriptor_set_path(descriptor_path)
        .compile_with_config(prost, &protos, &[proto_root, vendored_include])
}

fn vendored_error(error: protoc_bin_vendored::Error) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, error)
}
