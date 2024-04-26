use std::thread;
use std::time::Duration;
use common::tokio;
use common::tokio::time::sleep;


fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.spawn(flume_channel());
    thread::sleep(Duration::from_secs(10));
}

async fn flume_channel() {
    let (tx, rx) = flume::unbounded::<i32>();
    let rx1 = rx.clone();
    let rx2 = rx.clone();

    thread::spawn(move || {
        for i in 0..10 {
            tx.send(i).unwrap();
        }
    });

    thread::spawn(move ||
        async move {
            let rx1 = rx1.clone();
            while let Ok(val) = rx1.recv_async().await {
                println!("Thread 1 received: {}", val);
            }
        });
    sleep(Duration::from_secs(8)).await;
    while let Ok(val) = rx2.recv_async().await {
        println!("Thread 2 received: {}", val);
    }
}

fn crossbeam_channel() {
    let (sender, receiver) = crossbeam_channel::unbounded();
    let receiver2 = receiver.clone();
    let receiver3 = receiver.clone();

    thread::spawn(move || {
        for i in 0..10 {
            let msg = format!("Hello, world! -- {}", i);
            sender.send(msg).unwrap();
        }
    });

    thread::spawn(move || {
        while let Ok(msg) = receiver2.recv() {
            println!("Thread 2 received: {}", msg);
        }
    });

    thread::spawn(move || {
        while let Ok(msg) = receiver3.recv() {
            println!("Thread 3 received: {}", msg);
        }
    });

    thread::sleep(Duration::from_secs(11))
}