use async_runtime::LocalDomain;
use std::rc::Rc;

fn main() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let captured = Rc::new(());
    let _ = spawner.spawn(async move {
        drop(captured);
    });
}
