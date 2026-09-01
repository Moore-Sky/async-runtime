use async_runtime::LocalDomain;
use std::rc::Rc;

fn main() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let value = Rc::new(());
    let _ = spawner.dispatch(move || drop(value));
}
