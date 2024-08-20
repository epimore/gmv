use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use common::err::{GlobalResult, TransError};
use common::log::error;

static INDEX: AtomicUsize = AtomicUsize::new(0);

pub fn dump(file_name: &str, data: &[u8], seq: bool) -> GlobalResult<()> {
    let path = "./dump";
    std::fs::create_dir_all(path).hand_log(|msg| error!("{msg}"))?;
    if seq {
        let i = INDEX.fetch_add(1, Ordering::SeqCst);
        let mut f = File::create(format!("{path}/{file_name}-{i}.dump")).hand_log(|msg| error!("{msg}"))?;
        f.write_all(data).hand_log(|msg| error!("{msg}"))?;
    } else {
        let mut f = OpenOptions::new().create(true).write(true).append(true).open(format!("{path}/{file_name}.dump")).hand_log(|msg| error!("{msg}"))?;
        f.write_all(data).hand_log(|msg| error!("{msg}"))?;
    }
    Ok(())
}