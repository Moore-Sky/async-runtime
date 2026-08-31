use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::rc::Rc;

fn main() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let captured = Rc::new(());
    let _ = runtime.spawn(Priority::Normal, async move {
        drop(captured);
    });
}
