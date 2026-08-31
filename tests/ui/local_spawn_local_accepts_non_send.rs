use async_runtime::LocalDomain;
use std::rc::Rc;

fn main() {
    let local = LocalDomain::new();
    let value = Rc::new(42_u32);
    let _ = local.spawn_local(async move { value });
}
