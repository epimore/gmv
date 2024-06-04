use std::time::Duration;
use rsip::Response;
use common::err::GlobalResult;
use common::tokio::sync::mpsc;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::time::Instant;
use crate::gb::handler::builder::RequestBuilder;
use crate::gb::shared::event::{Container, EventSession};
use crate::gb::shared::rw::RequestOutput;

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    pub async fn query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn lazy_query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
    pub async fn lazy_query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
    pub async fn lazy_subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
}

pub struct CmdControl;

pub struct CmdNotify;

pub struct CmdStream;

impl CmdStream {
    pub async fn play_live() -> GlobalResult<Receiver<(Option<Response>, Instant)>> {
        // let (ident, msg) = RequestBuilder::play_live_request(&"".to_string(), &"".to_string(), "", 0, StreamMode::Udp, "").expect("TODO: panic message");
        let (tx, rx) = mpsc::channel(100);
        // RequestOutput::new(ident, msg, Some(tx)).do_send().await?;
        Ok(rx)
    }
}
