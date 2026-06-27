use base::serde::Deserialize;
use std::net::IpAddr;

#[derive(Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct Tcp {
    ip: IpAddr,
    port: u16,
}
