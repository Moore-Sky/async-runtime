use async_runtime::LocalDomain;

fn assert_sync<T: Sync>() {}

fn main() {
    assert_sync::<LocalDomain>();
}
