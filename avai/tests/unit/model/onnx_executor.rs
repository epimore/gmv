use super::*;

#[tokio::test]
async fn shutdown_reports_incomplete_for_non_cooperative_native_job() {
    let executor = NativeExecutor::new(1, 1).unwrap();
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    executor
        .submit(Box::new(move || {
            started_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
        }))
        .unwrap();
    started_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let shutdown = base::tokio::time::timeout(
        Duration::from_secs(1),
        executor.close_and_wait(Instant::now() + Duration::from_millis(20)),
    )
    .await;
    release_sender.send(()).unwrap();
    let error = shutdown
        .expect("bounded native shutdown detector hung")
        .unwrap_err();
    assert_eq!(error.code, "model_runtime_shutdown_incomplete");

    executor
        .close_and_wait(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}
