//! The tokio runtime everything off-thread runs on.
//!
//! GPUI has its own executor and it is not tokio's. `reqwest` needs a tokio
//! reactor: without one, every request panics with "there is no reactor running"
//! *inside* reqwest, which reads as a transport bug rather than a missing
//! runtime.
//!
//! Worker count is deliberate. Two threads: one drives the reactor, one absorbs
//! blocking waits. One cannot make progress while blocked, which would serialise
//! every concurrent request.
//!
//! Recording is heavier than HTTP, so this runtime also hosts the recognition
//! work. It does that on `spawn_blocking`, never on a worker, because whisper is
//! a long C call and holding a worker through it would stall the WebSocket the
//! overlay is reading from.

use std::sync::LazyLock;

use tokio::runtime::{Builder, Runtime};

/// The shared runtime, built on first use.
///
/// `LazyLock` rather than a `static` with a `OnceLock` dance: it is `Sync`,
/// and an app that never listens should not start threads for it.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .expect("the tokio runtime is the app's only async host; it must build")
});

/// A handle to the shared runtime.
pub fn handle() -> tokio::runtime::Handle {
    RUNTIME.handle().clone()
}

/// Run a future on the shared runtime without awaiting it here.
///
/// The seam between tokio and GPUI: a `tokio::spawn` from the UI thread returns a
/// `JoinHandle` that must be kept alive, and GPUI has its own `Task` for work
/// that touches the window. This is for work that does not.
pub fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    RUNTIME.spawn(fut);
}

/// Block on a future from a context that is not already async.
///
/// Only for startup, before the GPUI application loop owns the thread. Calling
/// this from inside a GPUI task would deadlock the UI thread.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_is_usable_and_shared() {
        // The same handle twice: the runtime must be built once, since a second
        // runtime would mean a second set of threads per call.
        let first = handle();
        let second = handle();

        // `Handle::id` is the runtime identity, so equal ids mean one runtime.
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn spawn_runs_work() {
        let (sender, receiver) = std::sync::mpsc::channel();

        spawn(async move {
            let _ = sender.send(42u32);
        });

        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("work should have run"),
            42
        );
    }

    #[test]
    fn blocking_work_does_not_stall_the_reactor() {
        // A long `spawn_blocking` must not prevent a short task from
        // completing, or the overlay's socket would freeze during recognition.
        let (sender, receiver) = std::sync::mpsc::channel();

        spawn(async move {
            let _ = tokio::task::spawn_blocking(|| {
                std::thread::sleep(std::time::Duration::from_millis(200));
            })
            .await;

            let _ = sender.send("free");
        });

        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("a request should complete while recognition runs"),
            "free"
        );
    }
}
