use async_runtime::LocalDomain;
use std::rc::Rc;

fn main() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let _ = spawner.spawn(async { Rc::new(()) });
}
