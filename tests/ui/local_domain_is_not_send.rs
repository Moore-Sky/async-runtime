use async_runtime::LocalDomain;

fn assert_send<T: Send>() {}

fn main() {
    assert_send::<LocalDomain>();
}
