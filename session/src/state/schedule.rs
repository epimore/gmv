use std::pin::Pin;
use std::sync::Arc;
use std::thread;

use cron::Schedule;

use base::chrono::Local;
use base::log::error;
use base::once_cell::sync::Lazy;
use base::tokio;
use base::tokio::sync::mpsc;
use base::tokio::sync::mpsc::{Receiver, Sender};
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;

pub trait ScheduleTask: Send + Sync + 'static {
    fn do_something(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

static SENDER: Lazy<Sender<(Schedule, Arc<dyn ScheduleTask>)>> = Lazy::new(|| {
    let (tx, rx) = mpsc::channel(100);
    let rt = GlobalRuntime::get_main_runtime();
    rt.rt_handle.spawn(run_scheduler(rx, rt.cancel));
    tx
});

/// 任务调度器
async fn run_scheduler(
    mut rx: Receiver<(Schedule, Arc<dyn ScheduleTask>)>,
    cancel_token: CancellationToken,
) {
    while let Some((schedule, task)) = rx.recv().await {
        if cancel_token.is_cancelled() {
            break;
        }
        tokio::spawn(run_task(schedule, task, cancel_token.child_token()));
    }
}

/// 运行任务
async fn run_task(
    schedule: Schedule,
    task: Arc<dyn ScheduleTask>,
    cancel_token: CancellationToken,
) {
    loop {
        if cancel_token.is_cancelled() {
            break;
        }
        let now = Local::now();
        if let Some(next_time) = schedule.upcoming(Local).next() {
            if let Ok(delay) = (next_time - now).to_std() {
                tokio::time::sleep(delay).await;
                task.do_something().await;
            }
        } else {
            error!("No upcoming schedule time found, exiting task.");
            break;
        }
    }
}

pub fn get_schedule_tx() -> Sender<(Schedule, Arc<dyn ScheduleTask>)> {
    SENDER.clone()
}

#[cfg(test)]
mod test {
    use std::pin::Pin;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::thread::sleep;
    use std::time::Duration;

    use cron::Schedule;

    use base::chrono::Local;

    use crate::state::schedule::{ScheduleTask, get_schedule_tx};

    struct MyTask;

    impl ScheduleTask for MyTask {
        fn do_something(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(async move {
                println!("Task executed at {:?}", Local::now());
            })
        }
    }

    // #[test]
    fn test() {
        let schedule = Schedule::from_str("*/5 * * * * *").unwrap(); // 每 5 秒执行一次
        let sender = get_schedule_tx();

        let task = Arc::new(MyTask);
        let _ = sender.try_send((schedule, task));

        sleep(Duration::from_secs(30));
        // 保持主线程运行
        // std::thread::park();
    }
}
