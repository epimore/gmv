use crate::general::util::Placeholder;
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::state::event::{Event, EventRes, OutEvent};
use crate::state::register::Register;
use base::bus::mpsc::TypedReceiver;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::tokio;
use base::tokio::fs;
use base::tokio::fs::File;
use base::tokio::io::AsyncWriteExt;
use base::tokio::sync::{broadcast, mpsc, oneshot};
use gmv_domain::enums::OptAction;
use gmv_domain::info::obj::StreamRecordInfo;
use gmv_domain::info::output::OutputEnum;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const STORE_MP4_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1));

pub enum Mp4OutputInnerEvent {
    StoreInfo(oneshot::Sender<StreamRecordInfo>), //获取当前录制信息
    Close,                                        //用于主动关闭录制
}

// pub struct Mp4StoreSender(pub oneshot::Sender<StreamRecordInfo>);
pub struct LocalStoreMp4Context {
    pub path: String,
    pub ssrc: u32,

    pub file_name: Arc<str>,                         //stream_id
    pub pkt_rx: broadcast::Receiver<Arc<MuxPacket>>, //数据接收端，当发送端drop，即录制完成
    pub record_event_tx: mpsc::Sender<(Event, Option<oneshot::Sender<EventRes>>)>, //用于主动发送录制报错、录制结束
    pub inner_event_rx: TypedReceiver<Mp4OutputInnerEvent>, //获取当前录制信息
    pub file_size: usize,
    pub ts: u64,   //second
    pub state: u8, //录制状态，0=进行，1=完成，2=录制部分，3=失败
}

impl LocalStoreMp4Context {
    pub fn store(mut self) {
        tokio::spawn(async move {
            Register::handle_stream_metadata_map_output(
                OptAction::Insert,
                &self.file_name,
                OutputEnum::LocalMp4,
            );
            match self.run().await {
                Ok(_) => {
                    let info = StreamRecordInfo {
                        stream_id: Some(self.file_name.to_string()),
                        path_file_name: Some(format!("{}/mp4/{}.mp4", self.path, self.file_name)),
                        file_size: self.file_size as u64,
                        timestamp: self.ts as u32,
                        state: if self.state == 0 { 1 } else { self.state },
                    };
                    let _ = self
                        .record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(info)), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
                Err(_) => {
                    let mut info = StreamRecordInfo::default();
                    info.stream_id = Some(self.file_name.to_string());
                    info.state = if self.state == 0 { 3 } else { self.state };
                    info.file_size = self.file_size as u64;
                    info.timestamp = self.ts as u32;
                    info.path_file_name = Some(format!("{}/mp4/{}.mp4", self.path, self.file_name));
                    let _ = self
                        .record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(info)), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
            }
            Register::handle_stream_metadata_map_output(
                OptAction::Remove,
                &self.file_name,
                OutputEnum::LocalMp4,
            );
        });
    }

    async fn run(&mut self) -> GlobalResult<()> {
        // 1. 创建目录
        let dir_path = Path::new(&self.path).join("mp4");
        fs::create_dir_all(&dir_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;

        // 2. 创建文件
        let file_path = dir_path.join(self.file_name.as_ref()).with_extension("mp4");
        let mut file = fs::File::create(&file_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;

        // 3. 处理第一个关键帧,并写入头信息
        if !self.handle_first_key_frame(&mut file).await? {
            return Ok(());
        }

        // 4. 持续接收数据包写入 + 监听录制过程信息获取事件
        let mut inner_closed = false;
        loop {
            tokio::select! {
                pkt_opt = self.pkt_rx.recv() => {
                    match pkt_opt {
                        Ok(pkt) =>{
                            file.write_all(&pkt.data).await.hand_log(|msg| error!("{msg}"))?;
                            self.ts = pkt.timestamp;
                            self.file_size += pkt.data.len();
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            self.state = 2;
                            warn!(
                                "mp4 recording incomplete: stage=recording, outcome=lagged, stream_id={}, ssrc={}, path={}, lost_packets={}",
                                self.file_name,
                                self.ssrc,
                                file_path.display(),
                                skipped
                            );
                            break;
                        }
                    }
                }
                inner_event_res = self.inner_event_rx.recv(), if !inner_closed => {
                    match inner_event_res {
                        Ok(inner_event) => match inner_event {
                            Mp4OutputInnerEvent::StoreInfo(record_info_tx) => {
                                let info = StreamRecordInfo { stream_id: Some(self.file_name.to_string()), path_file_name: None, file_size: self.file_size as u64, timestamp: self.ts as u32, state: self.state };
                                let _ = record_info_tx.send(info);
                            }
                            Mp4OutputInnerEvent::Close => {break;}
                        },
                        Err(_) => inner_closed = true,
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_first_key_frame(&mut self, file: &mut File) -> GlobalResult<bool> {
        let mut inner_closed = false;
        loop {
            tokio::select! {
                pkt_opt = self.pkt_rx.recv() => {
                    match pkt_opt {
                        Ok(pkt) =>{
                           if pkt.is_key {
                                // 写入文件头
                                let (tx, rx) = oneshot::channel();
                                Register::try_publish_mpsc(self.ssrc, ContextEvent::Inner(InnerEvent::Mp4Header(tx)))?;
                                let header = rx.await.hand_log(|msg| error!("{msg}"))?;
                                file.write_all(&header).await.hand_log(|msg| error!("{msg}"))?;

                                // 写入第一个关键帧
                                file.write_all(&pkt.data).await.hand_log(|msg| error!("{msg}"))?;
                                self.ts = pkt.timestamp;
                                self.file_size += pkt.data.len();
                                return Ok(true);
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.state = 3;
                            return Ok(false);
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            self.state = 2;
                            warn!(
                                "mp4 recording incomplete: stage=first_keyframe, outcome=lagged, stream_id={}, ssrc={}, lost_packets={}",
                                self.file_name,
                                self.ssrc,
                                skipped
                            );
                            return Ok(false);
                        }
                    }
                }
                inner_event_res = self.inner_event_rx.recv(), if !inner_closed => {
                    match inner_event_res {
                        Ok(inner_event) => match inner_event {
                            Mp4OutputInnerEvent::StoreInfo(record_info_tx) => {
                                let info = StreamRecordInfo { stream_id: Some(self.file_name.to_string()), path_file_name: None, file_size: self.file_size as u64, timestamp: self.ts as u32, state: self.state };
                                let _ = record_info_tx.send(info);
                            }
                            Mp4OutputInnerEvent::Close => {
                                self.state = 3;
                                return Ok(false);
                            }
                        },
                        Err(_) => inner_closed = true,
                    }
                }
            }
        }
    }
}
