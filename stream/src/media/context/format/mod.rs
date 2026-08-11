use crate::media::context::format::demuxer::{DemuxerContext, OutputTrackSource};
use axum::body::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use base::tokio::sync::{broadcast, watch};
use parking_lot::Mutex;
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVMediaType, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational, AVSampleFormat_AV_SAMPLE_FMT_FLTP,
    av_channel_layout_default, av_mallocz, avcodec_parameters_copy, avformat_new_stream,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub mod dashmp4;
pub mod demuxer;
pub mod flv;
pub mod fmp4;
pub mod h265flv;
mod hls_ts;
pub mod hlsfmp4;
pub mod mp4;
pub mod muxer;
mod ps;
pub mod rtp;
pub mod ts;

pub const OUTPUT_AAC_SAMPLE_RATE: i32 = 48_000;
pub const OUTPUT_AAC_CHANNELS: i32 = 1;
pub const OUTPUT_AAC_BIT_RATE: i64 = 48_000;

#[derive(Clone, Copy, Debug)]
pub struct PlannedStream {
    pub output_index: i32,
    pub input_time_base: AVRational,
    pub media_type: AVMediaType,
}

pub type PlannedStreamMap = HashMap<i32, PlannedStream>;

pub unsafe fn copy_output_plan(
    demuxer: &DemuxerContext,
    output: *mut rsmpeg::ffi::AVFormatContext,
) -> GlobalResult<(PlannedStreamMap, i32)> {
    let mut mapping = HashMap::with_capacity(demuxer.output_plan.tracks.len());
    let mut video_packet_index = -1;
    for track in &demuxer.output_plan.tracks {
        let output_stream = unsafe { avformat_new_stream(output, std::ptr::null()) };
        if output_stream.is_null() {
            return Err(GlobalError::new_sys_error(
                "failed to create planned output stream",
                |message| error!("{message}"),
            ));
        }
        let input_time_base = match track.source {
            OutputTrackSource::Input(index) => {
                let input_stream = unsafe { *(*demuxer.avio.fmt_ctx).streams.add(index) };
                if input_stream.is_null() || unsafe { (*input_stream).codecpar.is_null() } {
                    return Err(GlobalError::new_sys_error(
                        "planned input stream is unavailable",
                        |message| error!("{message}"),
                    ));
                }
                let ret = unsafe {
                    avcodec_parameters_copy((*output_stream).codecpar, (*input_stream).codecpar)
                };
                if ret < 0 {
                    return Err(GlobalError::new_sys_error(
                        "failed to copy planned stream parameters",
                        |message| error!("{message}: ffmpeg_code={ret}"),
                    ));
                }
                unsafe { (*input_stream).time_base }
            }
            OutputTrackSource::TranscodedAac(_) | OutputTrackSource::SilentAac => {
                unsafe { configure_fixed_aac_stream(output_stream)? };
                AVRational {
                    num: 1,
                    den: OUTPUT_AAC_SAMPLE_RATE,
                }
            }
        };
        unsafe {
            (*output_stream).time_base = input_time_base;
            (*(*output_stream).codecpar).codec_tag = 0;
        }
        let output_index = unsafe { (*output_stream).index };
        mapping.insert(
            track.packet_index,
            PlannedStream {
                output_index,
                input_time_base,
                media_type: track.media_type,
            },
        );
        if track.media_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
            video_packet_index = track.packet_index;
        }
    }
    Ok((mapping, video_packet_index))
}

unsafe fn configure_fixed_aac_stream(stream: *mut rsmpeg::ffi::AVStream) -> GlobalResult<()> {
    let codecpar = unsafe { (*stream).codecpar };
    unsafe {
        (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
        (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_AAC;
        (*codecpar).format = AVSampleFormat_AV_SAMPLE_FMT_FLTP as i32;
        (*codecpar).bit_rate = OUTPUT_AAC_BIT_RATE;
        (*codecpar).sample_rate = OUTPUT_AAC_SAMPLE_RATE;
        (*codecpar).channels = OUTPUT_AAC_CHANNELS;
        (*codecpar).channel_layout = 4;
        av_channel_layout_default(&mut (*codecpar).ch_layout, OUTPUT_AAC_CHANNELS);
        (*codecpar).frame_size = 1024;
        let extradata = av_mallocz(2 + 64) as *mut u8;
        if extradata.is_null() {
            return Err(GlobalError::new_sys_error(
                "failed to allocate silent AAC configuration",
                |message| error!("{message}"),
            ));
        }
        *extradata = 0x11;
        *extradata.add(1) = 0x88;
        (*codecpar).extradata = extradata;
        (*codecpar).extradata_size = 2;
    }
    Ok(())
}

pub struct MuxPacket {
    pub data: Bytes,
    pub is_key: bool,
    pub timestamp: u64,
    pub epoch: Instant,
    pub seq: usize,
    pub hls: Option<HlsPart>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlsPart {
    pub segment_seq: usize,
    pub part_seq: usize,
    pub duration_us: u64,
    pub segment_complete: bool,
    pub init_segment: Option<Bytes>,
}

const MUX_PACKET_REPLAY_LIMIT: usize = 256;
const MUX_PACKET_REPLAY_BYTES_LIMIT: usize = 16 * 1024 * 1024;
static NEXT_MUX_PACKET_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct MuxPacketReplay {
    epoch: Option<Instant>,
    bytes: usize,
    packets: VecDeque<Arc<MuxPacket>>,
}

impl MuxPacketReplay {
    fn clear(&mut self) {
        self.bytes = 0;
        self.packets.clear();
    }
}

struct MuxPacketChannel {
    id: u64,
    tx: broadcast::Sender<Arc<MuxPacket>>,
    replay: Mutex<MuxPacketReplay>,
    close: watch::Sender<bool>,
    published: AtomicU64,
}

#[derive(Clone)]
pub struct MuxPacketSender {
    inner: Arc<MuxPacketChannel>,
}

impl MuxPacketSender {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        let (close, _) = watch::channel(false);
        Self {
            inner: Arc::new(MuxPacketChannel {
                id: NEXT_MUX_PACKET_CHANNEL_ID.fetch_add(1, Ordering::Relaxed),
                tx,
                replay: Mutex::new(MuxPacketReplay::default()),
                close,
                published: AtomicU64::new(0),
            }),
        }
    }

    pub fn send(
        &self,
        packet: Arc<MuxPacket>,
    ) -> Result<usize, broadcast::error::SendError<Arc<MuxPacket>>> {
        let mut replay = self.inner.replay.lock();
        if *self.inner.close.borrow() {
            return Ok(0);
        }
        if replay.epoch.is_some_and(|epoch| epoch != packet.epoch) {
            replay.clear();
        }
        replay.epoch = Some(packet.epoch);
        self.inner.published.fetch_add(1, Ordering::Relaxed);

        if packet.is_key {
            replay.clear();
            if packet.data.len() <= MUX_PACKET_REPLAY_BYTES_LIMIT {
                replay.bytes = packet.data.len();
                replay.packets.push_back(packet.clone());
            }
        } else if !replay.packets.is_empty() {
            let next_bytes = replay.bytes.saturating_add(packet.data.len());
            if replay.packets.len() >= MUX_PACKET_REPLAY_LIMIT
                || next_bytes > MUX_PACKET_REPLAY_BYTES_LIMIT
            {
                replay.clear();
            } else {
                replay.bytes = next_bytes;
                replay.packets.push_back(packet.clone());
            }
        }

        match self.inner.tx.send(packet) {
            Ok(receiver_count) => Ok(receiver_count),
            Err(_) => Ok(0),
        }
    }

    pub fn subscribe(&self) -> MuxPacketReceiver {
        let replay = self.inner.replay.lock();
        let rx = self.inner.tx.subscribe();
        MuxPacketReceiver {
            channel_id: self.inner.id,
            replay: replay.packets.clone(),
            rx,
            close: self.inner.close.subscribe(),
        }
    }

    pub fn has_published(&self) -> bool {
        self.inner.published.load(Ordering::Relaxed) > 0
    }

    pub fn close(&self) {
        if self.inner.close.send_replace(true) {
            return;
        }
        self.inner.replay.lock().clear();
    }
}

pub struct MuxPacketReceiver {
    channel_id: u64,
    replay: VecDeque<Arc<MuxPacket>>,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    close: watch::Receiver<bool>,
}

impl MuxPacketReceiver {
    pub async fn recv(&mut self) -> Result<Arc<MuxPacket>, broadcast::error::RecvError> {
        if *self.close.borrow() {
            return Err(broadcast::error::RecvError::Closed);
        }
        if let Some(packet) = self.replay.pop_front() {
            return if *self.close.borrow() {
                Err(broadcast::error::RecvError::Closed)
            } else {
                Ok(packet)
            };
        }
        if *self.close.borrow() {
            return Err(broadcast::error::RecvError::Closed);
        }
        let result = base::tokio::select! {
            _ = self.close.changed() => Err(broadcast::error::RecvError::Closed),
            result = self.rx.recv() => result,
        };
        if *self.close.borrow() {
            Err(broadcast::error::RecvError::Closed)
        } else {
            result
        }
    }

    pub fn try_recv(&mut self) -> Result<Arc<MuxPacket>, broadcast::error::TryRecvError> {
        if *self.close.borrow() {
            return Err(broadcast::error::TryRecvError::Closed);
        }
        if let Some(packet) = self.replay.pop_front() {
            return if *self.close.borrow() {
                Err(broadcast::error::TryRecvError::Closed)
            } else {
                Ok(packet)
            };
        }
        self.rx.try_recv()
    }

    pub fn channel_id(&self) -> u64 {
        self.channel_id
    }
}

pub trait FmtMuxer {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: MuxPacketSender,
    ) -> GlobalResult<Self>
    where
        Self: Sized;
    fn get_header(&self) -> Bytes;
    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()>;
    fn flush(&mut self);
}

pub(super) fn can_start_fragmented_output(
    started: bool,
    video_stream_index: c_int,
    packet_stream_index: c_int,
    is_keyframe: bool,
) -> bool {
    started || video_stream_index < 0 || (packet_stream_index == video_stream_index && is_keyframe)
}

pub unsafe extern "C" fn write_callback(
    opaque: *mut c_void,
    buf: *mut u8,
    buf_size: c_int,
) -> c_int {
    unsafe {
        if opaque.is_null() || buf.is_null() || buf_size <= 0 {
            return buf_size;
        }
        let out_vec: &mut Vec<u8> = &mut *(opaque as *mut Vec<u8>);
        let old_len = out_vec.len();
        out_vec.reserve(buf_size as usize);
        std::ptr::copy_nonoverlapping(buf, out_vec.as_mut_ptr().add(old_len), buf_size as usize);
        out_vec.set_len(old_len + buf_size as usize);
        buf_size
    }
}

#[cfg(test)]
mod tests {
    use super::{MUX_PACKET_REPLAY_LIMIT, MuxPacket, MuxPacketSender, can_start_fragmented_output};
    use axum::body::Bytes;
    use base::tokio::sync::broadcast;
    use std::sync::Arc;
    use std::time::Instant;

    fn packet(epoch: Instant, seq: usize, is_key: bool) -> Arc<MuxPacket> {
        Arc::new(MuxPacket {
            data: Bytes::from(vec![seq as u8]),
            is_key,
            timestamp: seq as u64,
            epoch,
            seq,
            hls: None,
        })
    }

    #[test]
    fn fragmented_video_output_waits_for_its_first_keyframe() {
        assert!(!can_start_fragmented_output(false, 0, 1, false));
        assert!(!can_start_fragmented_output(false, 0, 0, false));
        assert!(can_start_fragmented_output(false, 0, 0, true));
        assert!(can_start_fragmented_output(true, 0, 1, false));
    }

    #[test]
    fn fragmented_audio_only_output_can_start_immediately() {
        assert!(can_start_fragmented_output(false, -1, 0, false));
    }

    #[test]
    fn late_subscribers_receive_the_latest_decodable_window() {
        let sender = MuxPacketSender::new(4);
        assert!(!sender.has_published());
        let epoch = Instant::now();
        let key = packet(epoch, 1, true);
        let delta = packet(epoch, 2, false);
        assert_eq!(sender.send(key.clone()).ok(), Some(0));
        assert!(sender.has_published());
        assert_eq!(sender.send(delta.clone()).ok(), Some(0));

        let mut first = sender.subscribe();
        let mut second = sender.subscribe();

        assert_eq!(first.replay.len(), 2);
        assert!(Arc::ptr_eq(first.replay.front().unwrap(), &key));
        assert_eq!(second.replay.len(), 2);
        assert!(Arc::ptr_eq(second.replay.back().unwrap(), &delta));

        assert!(Arc::ptr_eq(&first.try_recv().unwrap(), &key));
        assert!(Arc::ptr_eq(&first.try_recv().unwrap(), &delta));
        assert!(matches!(
            first.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let live = packet(epoch, 3, false);
        assert!(sender.send(live.clone()).is_ok());
        assert!(Arc::ptr_eq(&first.try_recv().unwrap(), &live));
        assert!(Arc::ptr_eq(&second.try_recv().unwrap(), &key));
    }

    #[test]
    fn explicit_close_discards_replay_and_wakes_existing_receivers() {
        base::tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(async {
                let sender = MuxPacketSender::new(4);
                assert!(sender.send(packet(Instant::now(), 1, true)).is_ok());
                let mut replay_receiver = sender.subscribe();
                let mut waiting_receiver = sender.subscribe();
                while waiting_receiver.try_recv().is_ok() {}
                let waiter = base::tokio::spawn(async move { waiting_receiver.recv().await });
                base::tokio::task::yield_now().await;

                sender.close();

                assert!(matches!(
                    replay_receiver.try_recv(),
                    Err(broadcast::error::TryRecvError::Closed)
                ));
                assert!(matches!(
                    waiter.await.expect("receiver task should finish"),
                    Err(broadcast::error::RecvError::Closed)
                ));
            });
    }

    #[test]
    fn replay_resets_on_keyframe_epoch_change_and_overflow() {
        let sender = MuxPacketSender::new(4);
        let first_epoch = Instant::now();
        let second_epoch = first_epoch + std::time::Duration::from_secs(1);
        let _ = sender.send(packet(first_epoch, 1, true));
        let _ = sender.send(packet(first_epoch, 2, false));
        let _ = sender.send(packet(second_epoch, 3, false));
        assert!(sender.subscribe().replay.is_empty());

        let key = packet(second_epoch, 4, true);
        let _ = sender.send(key);
        for seq in 5..=(MUX_PACKET_REPLAY_LIMIT + 4) {
            let _ = sender.send(packet(second_epoch, seq, false));
        }
        assert!(sender.subscribe().replay.is_empty());

        let replacement = packet(second_epoch, 999, true);
        let _ = sender.send(replacement.clone());
        let replay = sender.subscribe().replay;
        assert_eq!(replay.len(), 1);
        assert!(Arc::ptr_eq(replay.front().unwrap(), &replacement));
    }
}
