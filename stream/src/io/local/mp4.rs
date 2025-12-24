use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use crate::io::event_handler::{Event, EventRes, OutEvent};
use crate::media::context::format::MuxPacket;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio;
use base::tokio::fs;
use base::tokio::io::AsyncWriteExt;
use base::tokio::sync::{broadcast, mpsc, oneshot};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use base::bus::mpsc::TypedReceiver;
use base::tokio::fs::File;
use shared::info::obj::StreamRecordInfo;
use shared::info::output::OutputEnum;
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::state::cache;

const STORE_MP4_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1));

pub enum Mp4OutputInnerEvent {
    StoreInfo(oneshot::Sender<StreamRecordInfo>), //获取当前录制信息
    Close,//用于主动关闭录制
}

// pub struct Mp4StoreSender(pub oneshot::Sender<StreamRecordInfo>);
pub struct LocalStoreMp4Context {
    pub path: String,
    pub ssrc: u32,
    
    pub file_name: String, //stream_id
    pub pkt_rx: broadcast::Receiver<Arc<MuxPacket>>, //数据接收端，当发送端drop，即录制完成
    pub record_event_tx: mpsc::Sender<(Event, Option<oneshot::Sender<EventRes>>)>, //用于主动发送录制报错、录制结束
    pub inner_event_rx: TypedReceiver<Mp4OutputInnerEvent>, //获取当前录制信息
    pub file_size: usize,
    pub ts: u64, //second
    pub state: u8, //录制状态，0=进行，1=完成，2=录制部分，3=失败

}

impl LocalStoreMp4Context {

    pub fn store(mut self) {
        tokio::spawn(async move {
            cache::update_token(&self.file_name, OutputEnum::LocalMp4, format!("store_mp4_{}",self.file_name), true, STORE_MP4_ADDR,None);
            match self.run().await {
                Ok(_) => {
                    let info = StreamRecordInfo{ path_file_name: Some(format!("{}/mp4/{}.mp4",self.path, self.file_name)),file_size: self.file_size as u64,timestamp: self.ts as u32, state: 1 };
                    let _ = self.record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(info)), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
                Err(_) => {
                    let mut info = StreamRecordInfo::default();
                    info.state = 3;
                    info.path_file_name = Some(format!("{}/mp4/{}.mp4",self.path, self.file_name));
                    let _ = self.record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(info)), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
            }
            cache::update_token(&self.file_name, OutputEnum::LocalMp4, format!("store_mp4_{}",self.file_name), false, STORE_MP4_ADDR,Some(Duration::from_millis(200)));
        });
    }

    async fn run(&mut self) -> GlobalResult<()> {

        // 1. 创建目录
        let dir_path = Path::new(&self.path).join("mp4");
        fs::create_dir_all(&dir_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;

        // 2. 创建文件
        let file_path = dir_path.join(&self.file_name).with_extension("mp4");
        let mut file = fs::File::create(&file_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;
        
        // 3. 处理第一个关键帧,并写入头信息
 self.handle_first_key_frame(&mut file).await?;

        // 4. 持续接收数据包写入 + 监听录制过程信息获取事件
        loop {
            tokio::select! {
                pkt_opt = self.pkt_rx.recv() => {
                    match pkt_opt {
                        Ok(pkt) =>{
                            file.write_all(&pkt.data).await.hand_log(|msg| error!("{msg}"))?;
                            self.ts = pkt.timestamp;
                            self.file_size += pkt.data.len();
                        }
                        Err(_) => break,//发送端drop，录制结束
                    }
                }
                inner_event_res = self.inner_event_rx.recv() => {
                    if let Ok(inner_event) = inner_event_res {
                        match inner_event {
                            Mp4OutputInnerEvent::StoreInfo(record_info_tx) => {
                                let info = StreamRecordInfo{path_file_name: None,file_size: self.file_size as u64,timestamp: self.ts as u32,state: self.state};
                                let _ = record_info_tx.send(info);
                            }
                            Mp4OutputInnerEvent::Close => {break;}
                        }
                    }
                }
            }
        }
        Ok(())
    }
    
    async fn handle_first_key_frame(&mut self, file:&mut File) -> GlobalResult<()>{
        loop {
            tokio::select! {
                pkt_opt = self.pkt_rx.recv() => {
                    match pkt_opt {
                        Ok(pkt) =>{
                           if pkt.is_key {
                                // 写入文件头
                                let (tx, rx) = oneshot::channel();
                                cache::try_publish_mpsc(&self.ssrc, ContextEvent::Inner(InnerEvent::Mp4Header(tx)))?;
                                let header = rx.await.hand_log(|msg| error!("{msg}"))?;
                                file.write_all(&header).await.hand_log(|msg| error!("{msg}"))?;
                
                                // 写入第一个关键帧
                                file.write_all(&pkt.data).await.hand_log(|msg| error!("{msg}"))?;
                                self.ts = pkt.timestamp;
                                self.file_size += pkt.data.len();
                                break;
                            }
                        }
                        Err(_) => break,//发送端drop，录制结束
                    }
                }
                inner_event_res = self.inner_event_rx.recv() => {
                    if let Ok(inner_event) = inner_event_res {
                        match inner_event {
                            Mp4OutputInnerEvent::StoreInfo(record_info_tx) => {
                                let info = StreamRecordInfo{path_file_name: None,file_size: self.file_size as u64,timestamp: self.ts as u32,state: self.state};
                                let _ = record_info_tx.send(info);
                            }
                            Mp4OutputInnerEvent::Close => {break;}
                        }
                    }
                }
            }
        }
        Ok(())
    }
}