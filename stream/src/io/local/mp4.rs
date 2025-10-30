use crate::io::event_handler::{Event, EventRes, OutEvent};
use crate::media::context::format::MuxPacket;
use base::chrono::Local;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio;
use base::tokio::fs;
use base::tokio::io::AsyncWriteExt;
use base::tokio::sync::{broadcast, mpsc, oneshot};
use std::path::Path;
use std::sync::Arc;
use base::bus::mpsc::TypedReceiver;
use base::tokio::sync::oneshot::Sender;
use shared::info::obj::StreamRecordInfo;

pub struct Mp4StoreSender(pub oneshot::Sender<StreamRecordInfo>);
pub struct LocalStoreMp4Context {
    pub path: String,
    
    pub file_name: String,
    pub pkt_rx: broadcast::Receiver<Arc<MuxPacket>>, //数据接收端，当发送端drop，即录制完成
    pub record_event_tx: mpsc::Sender<(Event, Option<oneshot::Sender<EventRes>>)>, //用于主动发送录制报错、录制结束
    pub record_info_event_rx: TypedReceiver<Mp4StoreSender>, //获取当前录制信息
    
    pub file_size: usize,
    pub ts: u64 //second
}

impl LocalStoreMp4Context {
    pub fn store(mut self) {
        tokio::spawn(async move {
            match self.run().await {
                Ok(_) => {
                    let info = StreamRecordInfo{file_name: Some(self.file_name),file_size: self.file_size as u64,timestamp: self.ts as u32,};
                    let _ = self.record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(info)), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
                Err(_) => {
                    let _ = self.record_event_tx
                        .send((Event::Out(OutEvent::EndRecord(StreamRecordInfo::default())), None))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
            }

        });
    }

    async fn run(&mut self) -> GlobalResult<()> {
        // 1. 创建目录
        let dir_path = Path::new(&self.path).join("mp4");
        fs::create_dir_all(&dir_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;

        // 2. 创建文件
        let file_path = dir_path.join(&self.file_name);
        let mut file = fs::File::create(&file_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;
        // 3. 持续接收数据包写入 + 监听录制过程信息获取事件
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
                record_info_tx = self.record_info_event_rx.recv() => {
                    if let Ok(record_info_tx) = record_info_tx {
                       let info = StreamRecordInfo{file_name: None,file_size: self.file_size as u64,timestamp: self.ts as u32,};
                        let _ = record_info_tx.0.send(info);
                    }
                }
            }
        }
        Ok(())
    }
}