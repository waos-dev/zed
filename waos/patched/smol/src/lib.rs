//! WaOS port, from the 10-16 experiment: smol 2.0.2's API, built from the same crates. On wasm it leaves out what
//! needs async-io's reactor, which has no WASI backend: `Async`, `Timer`, `net`
//! and `process`; `block_on` there is futures-lite's, which parks the thread instead of
//! driving a reactor.

pub use async_executor::{Executor, LocalExecutor, Task};
pub use blocking::{unblock, Unblock};
pub use futures_lite::{future, io, pin, prelude, ready, stream};
pub use {async_channel as channel, async_fs as fs, async_lock as lock};

#[cfg(not(target_os = "wasi"))]
pub use {
    async_io::{block_on, Async, Timer},
    async_net as net, async_process as process,
};

#[cfg(target_os = "wasi")]
pub use futures_lite::future::block_on;

mod spawn;
pub use spawn::spawn;
