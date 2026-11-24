//! Abort signals for long wallet calls.

use std::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use tokio::sync::watch;

/// The sender side of an [`AbortSignal`].
#[derive(Debug)]
pub struct AbortController {
    tx: watch::Sender<bool>,
}

impl AbortController {
    /// Create a controller whose signal has not fired.
    pub fn new() -> Self {
        Self {
            tx: watch::Sender::new(false),
        }
    }

    /// Get a signal of this controller.
    pub fn signal(&self) -> AbortSignal {
        AbortSignal {
            rx: self.tx.subscribe(),
        }
    }

    /// Fire the signal.
    pub fn abort(&self) {
        self.tx.send_replace(true);
    }
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

/// A signal that stops a wallet call when its [`AbortController`] fires.
#[derive(Clone, Debug)]
pub struct AbortSignal {
    rx: watch::Receiver<bool>,
}

impl AbortSignal {
    /// Return true when the signal has fired.
    pub fn aborted(&self) -> bool {
        *self.rx.borrow()
    }

    async fn fired(&self) {
        let mut rx = self.rx.clone();
        if rx.wait_for(|a| *a).await.is_err() {
            std::future::pending::<()>().await
        }
    }
}

/// The signal of the current call fired.
pub(crate) struct Aborted;

struct Scope {
    signal: AbortSignal,
    writes: Cell<u32>,
}

tokio::task_local! {
    static SCOPE: Option<Scope>;
}

/// Run `fut` with `signal` as the signal of the current call.
pub(crate) async fn scope<F: Future>(signal: Option<AbortSignal>, fut: F) -> F::Output {
    let scope = signal.map(|signal| Scope {
        signal,
        writes: Cell::new(0),
    });
    SCOPE.scope(scope, fut).await
}

fn with<R>(f: impl FnOnce(&Scope) -> R) -> Option<R> {
    SCOPE.try_with(|s| s.as_ref().map(f)).ok().flatten()
}

/// Return true when the signal of the current call has fired.
pub(crate) fn fired() -> bool {
    with(|s| s.signal.aborted()).unwrap_or(false)
}

pub(crate) fn check() -> Result<(), Aborted> {
    if fired() { Err(Aborted) } else { Ok(()) }
}

/// Return the number of writes the current call started.
pub(crate) fn writes() -> u32 {
    with(|s| s.writes.get()).unwrap_or(0)
}

/// Return true when the current call started a write.
pub(crate) fn wrote() -> bool {
    writes() > 0
}

/// Run a read. When the signal fires, drop it and return [`Aborted`].
pub(crate) async fn race<T, E: From<Aborted>>(
    fut: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let Some(signal) = with(|s| s.signal.clone()) else {
        return fut.await;
    };
    if signal.aborted() {
        return Err(Aborted.into());
    }
    let mut fired = pin!(signal.fired());
    let mut fut = pin!(fut);
    poll_fn(|cx| {
        if fired.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Aborted.into()));
        }
        fut.as_mut().poll(cx)
    })
    .await
}

/// Run a write. Count it as started, then race it as a read.
pub(crate) async fn write<T, E: From<Aborted>>(
    fut: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    check()?;
    with(|s| s.writes.set(s.writes.get() + 1));
    race(fut).await
}

/// Run `fut` out of reach of the signal.
pub(crate) async fn shield<F: Future>(fut: F) -> F::Output {
    SCOPE.scope(None, fut).await
}
