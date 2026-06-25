use prost::Message;
use prost_types::FileDescriptorSet;

fn descriptor() -> FileDescriptorSet {
    FileDescriptorSet::decode(gmv_protocol::FILE_DESCRIPTOR_SET).unwrap()
}

#[test]
fn descriptor_contains_versioned_packages() {
    let descriptor = descriptor();
    let packages = descriptor
        .file
        .iter()
        .map(|file| file.package.as_deref().unwrap_or_default())
        .collect::<Vec<_>>();

    for package in [
        "gmv.common.v1",
        "gmv.guard.v1",
        "gmv.session.v1",
        "gmv.stream.v1",
        "gmv.avai.v1",
    ] {
        assert!(packages.contains(&package), "missing package {package}");
    }
}

#[test]
fn node_identity_contains_instance_id_fencing_token() {
    let descriptor = descriptor();
    let common = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.common.v1"))
        .unwrap();
    let node_identity = common
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("NodeIdentity"))
        .unwrap();
    let instance_id = node_identity
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some("instance_id"))
        .unwrap();

    assert_eq!(instance_id.number, Some(2));
}

#[test]
fn enums_start_with_unspecified_zero_value() {
    let descriptor = descriptor();

    for file in descriptor.file {
        for item in file.enum_type {
            let enum_name = item.name.unwrap_or_default();
            let first = item
                .value
                .first()
                .unwrap_or_else(|| panic!("enum {enum_name} in {:?} has no values", file.name));
            assert_eq!(
                first.number,
                Some(0),
                "enum {enum_name} first value is not 0"
            );
            assert!(
                first
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .ends_with("UNSPECIFIED"),
                "enum {enum_name} first value must end with UNSPECIFIED"
            );
        }
    }
}

#[test]
fn guard_and_direct_service_rpc_boundaries_exist() {
    let descriptor = descriptor();
    let services = descriptor
        .file
        .iter()
        .flat_map(|file| {
            let package = file.package.clone().unwrap_or_default();
            file.service.iter().map(move |service| {
                format!("{package}.{}", service.name.as_deref().unwrap_or_default())
            })
        })
        .collect::<Vec<_>>();

    for service in [
        "gmv.guard.v1.GuardNodeControl",
        "gmv.guard.v1.GuardControl",
        "gmv.session.v1.SessionControl",
        "gmv.stream.v1.StreamControl",
        "gmv.avai.v1.AvaiControl",
    ] {
        assert!(
            services.contains(&service.to_string()),
            "missing service {service}"
        );
    }
}
