use std::net::IpAddr;
use base::serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct Udp {
    ip: IpAddr,
    port: u16,
}