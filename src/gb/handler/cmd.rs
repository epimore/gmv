use std::time::Duration;
use common::err::GlobalResult;
use common::tokio::time::Instant;
use crate::gb::handler::builder::RequestBuilder;
use crate::gb::shard::event::{Container, EventSession};
use crate::gb::shard::rw::RequestOutput;

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    pub async fn query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id)?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id)?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id)?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn lazy_query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id)?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }
    pub async fn lazy_query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id)?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }
    pub async fn lazy_subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id)?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }
}

pub struct CmdControl;

pub struct CmdNotify;