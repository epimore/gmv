use base::serde::Deserialize;
use std::net::IpAddr;

#[derive(Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct Udp {
    ip: IpAddr,
    port: u16,
}
