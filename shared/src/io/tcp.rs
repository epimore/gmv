use std::net::IpAddr;
use base::serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct Tcp {
    ip: IpAddr,
    port: u16,
}