# async-runtime 最简 API 模型

```rust
// 1. 其余线程：通用 worker pool
let runtime = RuntimeBuilder::new(NonZeroUsize::new(6).unwrap()).build()?;
let general = runtime.spawner();

// 2. 当前主线程：Render Loop
let render_rt = LocalDomain::new();
let render = render_rt.spawner();

// 3. 线程 1：Logic
let (send_logic, recv_logic) = std::sync::mpsc::sync_channel(1);
let (stop_logic, wait_logic_stop) = async_channel::bounded(1);

let logic_thread = std::thread::spawn(move || {
    // LocalDomain 必须在实际执行任务的线程内创建
    let local_rt = LocalDomain::new();
    send_logic.send(local_rt.spawner()).unwrap();

    // Logic 线程持续驱动自己的任务，直到收到退出信号
    async_io::block_on(local_rt.run(async {
        let _ = wait_logic_stop.recv().await;
    }));

    async_io::block_on(local_rt.shutdown_graceful());
});

let logic = recv_logic.recv()?;

// 调用方只保存三个任务投递句柄
let domains = Domains { render, logic, general };

domains.general.spawn(Priority::Background, async {
    load_resource().await
})?.detach();

domains.logic.spawn(async {
    run_logic()
})?.detach();

// Render 线程内可提交使用 Rc 等 !Send 数据的任务
render_rt.spawn_local(async {
    update_render_state()
})?.detach();

// 4. 主线程每帧非阻塞地驱动 Render 域
while running {
    while render_rt.try_tick() {}
    render_frame();
}

// 5. 关闭各执行域
async_io::block_on(render_rt.shutdown_graceful());
stop_logic.send_blocking(())?;
logic_thread.join().unwrap();
runtime.shutdown_graceful()?;
```

```rust
struct Domains {
    render: LocalSpawner, // 当前主线程 / Render Loop
    logic: LocalSpawner,  // 线程 1 / Logic
    general: Spawner,     // 其余通用 worker
}
```

每个固定线程拥有一个 `LocalDomain`。crate 不内建 Render、Logic 等业务名称，业务层通过
`LocalSpawner` 区分执行域；没有线程亲和要求的任务统一提交给 `Spawner`。
