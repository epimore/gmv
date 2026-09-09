pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gmv_protocol_descriptor.bin"));

pub mod common {
    pub mod v1 {
        tonic::include_proto!("gmv.common.v1");
    }
}

pub mod guard {
    pub mod v1 {
        tonic::include_proto!("gmv.guard.v1");
    }
}

pub mod session {
    pub mod v1 {
        tonic::include_proto!("gmv.session.v1");
    }
}

pub mod stream {
    pub mod v1 {
        tonic::include_proto!("gmv.stream.v1");
    }
}

pub mod avai {
    pub mod v1 {
        tonic::include_proto!("gmv.avai.v1");
    }
}

pub mod gmv_center_agent {
    #[allow(clippy::large_enum_variant)]
    pub mod v1 {
        tonic::include_proto!("gmv.center_agent.v1");
    }
}

pub mod gmv_center_agent_mqtt;
