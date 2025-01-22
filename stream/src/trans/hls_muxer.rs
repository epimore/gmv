#![allow(warnings)]
use common::exception::{GlobalResult, TransError};
use common::log::error;
use crate::coder::FrameData;
use crate::state::cache;


pub fn run(ssrc: u32, rx: crossbeam_channel::Receiver<FrameData>) {
    if let Some(tx) = cache::get_hls_tx(&ssrc) {}
}

fn create_dir(ssrc: u32) -> GlobalResult<()> {
    let path = std::path::Path::new("./hls");
    let mut path_buf = path.to_path_buf();
    let date_str = common::chrono::Local::now().date_naive().format("%Y-%m-%d");
    path_buf.push(format!("/{}", date_str));

    std::fs::create_dir_all(path).hand_log(|msg| error!("create log dir failed: {msg}"))?;
    Ok(())
}