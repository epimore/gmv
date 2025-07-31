use std::net::IpAddr;
use common::serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct Tcp {
    ip: IpAddr,
    port: u16,
}