use crate::general::util::Placeholder;
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacketReceiver;
use crate::state::event::{Event, EventRes, OutEvent};
use crate::state::register::Register;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use base::bus::mpsc::TypedReceiver;
use base::dashmap::DashMap;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::once_cell::sync::Lazy;
use base::tokio;
use base::tokio::fs;
use base::tokio::fs::File;
use base::tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use base::tokio::sync::{broadcast, mpsc, oneshot};
use base::tokio_util::io::ReaderStream;
use gmv_domain::enums::OptAction;
use gmv_domain::info::obj::StreamRecordInfo;
use gmv_domain::info::output::OutputEnum;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const STORE_MP4_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1));

struct CompletedMp4 {
    path: PathBuf,
    token: Option<String>,
}

static COMPLETED_MP4: Lazy<DashMap<String, CompletedMp4>> = Lazy::new(DashMap::new);

pub enum Mp4OutputInnerEvent {
    StoreInfo(oneshot::Sender<StreamRecordInfo>), //获取当前录制信息
    Close,                                        //用于主动关闭录制
}

// pub struct Mp4StoreSender(pub oneshot::Sender<StreamRecordInfo>);
pub struct LocalStoreMp4Context {
    pub path: String,
    pub token: Option<String>,
    pub ssrc: u32,

    pub stream_id: Arc<str>,
    pub file_name: Arc<str>, //云端录像 task_id；旧调用默认为 stream_id
    pub session_hook_endpoint: Option<String>,
    pub min_free_bytes: u64,
    pub pkt_rx: MuxPacketReceiver, //数据接收端，当发送端drop，即录制完成
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
                    let path_file_name = format!("{}/mp4/{}.mp4", self.path, self.file_name);
                    if self.state == 0 {
                        COMPLETED_MP4.insert(
                            self.file_name.to_string(),
                            CompletedMp4 {
                                path: PathBuf::from(&path_file_name),
                                token: self.token.clone(),
                            },
                        );
                    }
                    let info = StreamRecordInfo {
                        stream_id: Some(self.stream_id.to_string()),
                        path_file_name: Some(path_file_name),
                        file_size: self.file_size as u64,
                        timestamp: self.ts as u32,
                        state: if self.state == 0 { 1 } else { self.state },
                    };
                    let _ = self
                        .record_event_tx
                        .send((
                            Event::Out(OutEvent::EndRecord(
                                info,
                                self.session_hook_endpoint.clone(),
                            )),
                            None,
                        ))
                        .await
                        .hand_log(|msg| error!("{msg}"));
                }
                Err(_) => {
                    let mut info = StreamRecordInfo::default();
                    info.stream_id = Some(self.stream_id.to_string());
                    info.state = if self.state == 0 { 3 } else { self.state };
                    info.file_size = self.file_size as u64;
                    info.timestamp = self.ts as u32;
                    info.path_file_name = Some(format!("{}/mp4/{}.mp4", self.path, self.file_name));
                    let _ = self
                        .record_event_tx
                        .send((
                            Event::Out(OutEvent::EndRecord(
                                info,
                                self.session_hook_endpoint.clone(),
                            )),
                            None,
                        ))
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
        let part_path = file_path.with_extension("mp4.part");
        let mut file = fs::File::create(&part_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;

        // 3. 处理第一个关键帧,并写入头信息
        if !self.handle_first_key_frame(&mut file).await? {
            drop(file);
            let _ = fs::remove_file(&part_path).await;
            return Ok(());
        }

        // 4. 持续接收数据包写入 + 监听录制过程信息获取事件
        let mut inner_closed = false;
        let mut last_space_check = Instant::now();
        loop {
            tokio::select! {
                pkt_opt = self.pkt_rx.recv() => {
                    match pkt_opt {
                        Ok(pkt) =>{
                            file.write_all(&pkt.data).await.hand_log(|msg| error!("{msg}"))?;
                            self.ts = pkt.timestamp;
                            self.file_size += pkt.data.len();
                            if self.min_free_bytes > 0 && last_space_check.elapsed() >= Duration::from_secs(5) {
                                last_space_check = Instant::now();
                                match fs2::available_space(&dir_path) {
                                    Ok(available) if available <= self.min_free_bytes => {
                                        self.state = 2;
                                        warn!(
                                            "mp4 recording stopped at storage watermark: stage=recording, outcome=partial, reason=disk_space_low, stream_id={}, ssrc={}, available_bytes={}, watermark_bytes={}",
                                            self.stream_id,
                                            self.ssrc,
                                            available,
                                            self.min_free_bytes
                                        );
                                        break;
                                    }
                                    Ok(_) => {}
                                    Err(err) => {
                                        self.state = 2;
                                        warn!(
                                            "mp4 storage availability check failed: stage=recording, outcome=partial, reason=storage_unavailable, stream_id={}, ssrc={}, err={err}",
                                            self.stream_id,
                                            self.ssrc
                                        );
                                        break;
                                    }
                                }
                            }
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
                                let info = StreamRecordInfo { stream_id: Some(self.stream_id.to_string()), path_file_name: None, file_size: self.file_size as u64, timestamp: self.ts as u32, state: self.state };
                                let _ = record_info_tx.send(info);
                            }
                            Mp4OutputInnerEvent::Close => {
                                self.state = 2;
                                break;
                            }
                        },
                        Err(_) => inner_closed = true,
                    }
                }
            }
        }
        file.flush().await.hand_log(|msg| error!("{msg}"))?;
        file.sync_all().await.hand_log(|msg| error!("{msg}"))?;
        drop(file);
        fs::rename(&part_path, &file_path)
            .await
            .hand_log(|msg| error!("{msg}"))?;
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
                                let info = StreamRecordInfo { stream_id: Some(self.stream_id.to_string()), path_file_name: None, file_size: self.file_size as u64, timestamp: self.ts as u32, state: self.state };
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

pub async fn serve_completed(stream_id: &str, token: &str, range: Option<&str>) -> Response<Body> {
    let Some(entry) = COMPLETED_MP4.get(stream_id) else {
        return status_response(StatusCode::NOT_FOUND);
    };
    if entry
        .token
        .as_deref()
        .is_some_and(|expected| expected != token)
    {
        return status_response(StatusCode::UNAUTHORIZED);
    }
    let path = entry.path.clone();
    drop(entry);

    let Ok(mut file) = File::open(path).await else {
        return status_response(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = file.metadata().await else {
        return status_response(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let total = metadata.len();
    if total == 0 {
        return status_response(StatusCode::NO_CONTENT);
    }

    let (status, start, end) = match range {
        Some(value) => match parse_range(value, total) {
            Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                    .body(Body::empty())
                    .unwrap();
            }
        },
        None => (StatusCode::OK, 0, total - 1),
    };
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return status_response(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let length = end - start + 1;
    let stream = ReaderStream::new(file.take(length));
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    builder.body(Body::from_stream(stream)).unwrap()
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(total);
        return (suffix > 0).then_some((total - suffix, total - 1));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

fn status_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn parses_http_byte_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1_000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1_000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1_000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1_000), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 1_000), None);
    }
}
