use std::{
    thread::{sleep, spawn},
    time::Duration,
};

use futures::executor::block_on;
use tokio::task::yield_now;

struct Foo;

async fn wait() {
    yield_now().await;
    sleep(Duration::from_secs(1))
}

impl Foo {
    async fn run(self) {
        let mut i = 0;
        loop {
            eprintln!("Iteration number: {}", i);
            i += 1;
            wait().await
        }
    }
}

fn cancelling() {
    let foo = Foo;
    let (cancel, handle) = lullable::lullable(foo.run());

    spawn(move || block_on(cancel));

    std::thread::sleep(Duration::from_secs(3));
    let pause = handle.pause();
    eprintln!("Task paused");
    std::thread::sleep(Duration::from_secs(10));
    drop(pause);

    eprintln!("Task unpaused");
    std::thread::sleep(Duration::from_secs(3));
    handle.abort();
    eprintln!("Task cancelled")
}

fn main() {
    cancelling()
}
