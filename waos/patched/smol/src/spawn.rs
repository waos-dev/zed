//! smol 2.0.2's global executor, as smol writes it, but initialized through std's `OnceLock`:
//! async-lock has no blocking `OnceCell` initialization on WASI.

use std::future::Future;
use std::panic::catch_unwind;
use std::sync::OnceLock;
use std::thread;

use async_executor::{Executor, Task};
use futures_lite::future;

use crate::block_on;

pub fn spawn<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    static GLOBAL: OnceLock<Executor<'static>> = OnceLock::new();

    fn global() -> &'static Executor<'static> {
        GLOBAL.get_or_init(|| {
            let num_threads = std::env::var("SMOL_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            for n in 1..=num_threads {
                thread::Builder::new()
                    .name(format!("smol-{}", n))
                    .spawn(|| loop {
                        catch_unwind(|| block_on(global().run(future::pending::<()>()))).ok();
                    })
                    .expect("cannot spawn executor thread");
            }
            let ex = Executor::new();
            #[cfg(not(target_os = "wasi"))]
            ex.spawn(async_process::driver()).detach();
            ex
        })
    }

    global().spawn(future)
}
