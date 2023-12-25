# Lullable (Experimental)

**Lull that task!**

```rust
use std::{
    thread::{sleep, spawn},
    time::Duration,
};

use futures::executor::block_on;
use tokio::task::yield_now;

fn main() {
    let (cancel, handle) = lullable::lullable(async {
        let mut i = 0;
        loop {
            eprintln!("Iteration number: {}", i);
            i += 1;
            yield_now().await;
            sleep(Duration::from_secs(1))
        }
    });

    spawn(move || block_on(cancel));

    std::thread::sleep(Duration::from_secs(3));
    let pause = handle.pause();
    eprintln!("Task paused");

    std::thread::sleep(Duration::from_secs(3));
    drop(pause);

    eprintln!("Task unpaused");
    std::thread::sleep(Duration::from_secs(3));
    handle.abort();
    eprintln!("Task cancelled")
}
```
