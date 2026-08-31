use async_runtime::{Priority, RuntimeBuilder};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn idle_workers_wake_for_timer() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move {
            async_io::Timer::after(Duration::from_millis(20)).await;
            tx.send(()).unwrap();
        })
        .unwrap()
        .detach();
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn idle_workers_wake_for_async_channel() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let (wake_tx, wake_rx) = async_channel::bounded(1);
    let task = runtime
        .spawn(
            Priority::Normal,
            async move { wake_rx.recv().await.unwrap() },
        )
        .unwrap();

    std::thread::sleep(Duration::from_millis(20));
    wake_tx.send_blocking(42_u8).unwrap();
    assert_eq!(async_io::block_on(task), 42);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn idle_workers_wake_for_loopback_io() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let listener = async_io::Async::<TcpListener>::bind(([127, 0, 0, 1], 0)).unwrap();
    let address = listener.get_ref().local_addr().unwrap();
    let accepted = runtime
        .spawn(Priority::Normal, async move {
            let (_stream, peer) = listener.accept().await.unwrap();
            peer.ip()
        })
        .unwrap();

    std::thread::sleep(Duration::from_millis(20));
    let client = TcpStream::connect(address).unwrap();
    assert!(async_io::block_on(accepted).is_loopback());
    drop(client);
    runtime.shutdown_graceful().unwrap();
}
