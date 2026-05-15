use std::sync::Arc;
use base::cache::c100k::CacheEvent;
use base::exception::GlobalResultExt;
use base::log::{error, warn};
use base::tokio;
use base::tokio::select;
use base::tokio::sync::mpsc::Receiver;
use base::tokio::sync::oneshot::Sender;
use base::tokio::sync::Semaphore;
use base::tokio_util::sync::CancellationToken;
use crate::register::core::{Inner, Register, TimeScheduleKey, SERVER_HEART_EXPIRE};
use crate::storage::entity::GmvDevice;

const MAX_WORKER_POOL: usize = 128;
#[derive(Clone, Eq, PartialEq)]
pub enum Event {
    DeviceOffline(Arc<str>), //设备心跳超时下线
    ServerHeart(Arc<str>),//服务自身心跳
    OutSession(u64),
}
#[derive(Clone, Eq, PartialEq)]
pub enum EventRes{}
pub async fn schedule_event(
    inner: Arc<Inner>,
    mut event_rx: Receiver<(Event, Option<Sender<EventRes>>)>,
    cancel_token: CancellationToken,
) {
    // let pretend = HttpClient::template()
    //     .expect("Http client template init failed");
    let semaphore = Arc::new(Semaphore::new(MAX_WORKER_POOL));
    loop {
        select! {
           biased; // 按编写顺序检查分支
            _ = on_time_schedule(&inner)=>{},
            _ = handle_rx_event(&mut event_rx,semaphore.clone()) => {}
            _ = cancel_token.cancelled() => {break;}
        }
    }
}

async fn handle_rx_event(rx: &mut Receiver<(Event, Option<Sender<EventRes>>)>,semaphore: Arc<Semaphore>,){
    if let Some((event, tx)) = rx.recv().await {
        if let Ok(permit) = semaphore
            .acquire_owned()
            .await
            .hand_log(|msg| error!("{msg}"))
        {
            tokio::spawn(async move{
                hand_event(event).await;
            });
            drop(permit);
        }
    }

}

async fn hand_event(event:Event){
    match event {
        Event::DeviceOffline(device_id) => {
            let _ = GmvDevice::update_gmv_device_status_by_device_id(device_id.as_ref(), 0).await;
        }
        Event::ServerHeart(domain_id) => {
           let _ = Register::server_keep_heart_update_db(domain_id).await;
        }
        Event::OutSession(_) => {}
    }
}

async fn on_time_schedule(inner: &Inner) {
    if let Some(batch) = inner.time_schedule.next_batch().await {
        for CacheEvent { key, version, .. } in batch {
            match key {
                TimeScheduleKey::Device3Heart(device_id) => {
                    warn!("设备 {} 心跳超时，移除设备", device_id);
                    Register::remove_device_by_inner(&device_id, inner);
                    let _ = inner
                        .event_tx
                        .try_send(Event::DeviceOffline(device_id))
                        .hand_log(|msg| error!("{msg}"));
                }
                TimeScheduleKey::OutSession(_) => {}
                TimeScheduleKey::ServerHeart(domain_id) => {
                    let _ = inner
                        .event_tx
                        .try_send(Event::ServerHeart(domain_id.clone()))
                        .hand_log(|msg| error!("{msg}"));
                    let _ = inner.time_schedule
                        .insert(TimeScheduleKey::ServerHeart(domain_id), SERVER_HEART_EXPIRE);
                }
                TimeScheduleKey::DeviceRegistration(device_id) => {
                    warn!("设备 {} 注册过期，移除设备", device_id);
                    Register::remove_device_by_inner(&device_id, inner);
                    let _ = inner
                        .event_tx
                        .try_send(Event::DeviceOffline(device_id))
                        .hand_log(|msg| error!("{msg}"));
                }
            }
        }
    }
}