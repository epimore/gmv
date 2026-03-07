use std::time::Duration;
use base::cache::Cacheable;
use base::exception::GlobalResultExt;
use futures_core::future::BoxFuture;
use log::error;
use shared::info::obj::StreamPlayInfo;
use shared::info::output::OutputEnum;
use crate::io::event_handler::{Event, OutEvent};
use crate::state::cache;

#[derive(Clone)]
pub struct CacheStreamUser {
    pub expires_ttl: Option<Duration>,
    pub stream_id: String,
    pub remote_addr: String,
    pub token: String,
}
impl Cacheable for CacheStreamUser {
    fn expire_call(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            if let Some((bsi, user_count)) =
                cache::get_base_stream_info_by_stream_id(&self.stream_id)
            {
                let info = StreamPlayInfo::new(
                    bsi,
                    self.remote_addr.clone(),
                    self.token.clone(),
                    OutputEnum::DashFmp4,
                    user_count,
                );
                let _ = cache::get_event_tx()
                    .send((Event::Out(OutEvent::OffPlay(info)), None))
                    .await
                    .hand_log(|msg| error!("event channel error: {msg}"));
            }
        })
    }

    fn expire_ttl(&self) -> Option<Duration> {
        self.expires_ttl.clone()
    }
}