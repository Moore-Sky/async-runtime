use async_runtime::LocalSpawner;

fn assert_send_sync<T: Send + Sync>() {}

fn main() {
    assert_send_sync::<LocalSpawner>();
}
