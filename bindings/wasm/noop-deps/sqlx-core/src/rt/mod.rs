use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

#[cfg(feature = "_rt-async-std")]
pub mod rt_async_std;

#[cfg(feature = "_rt-tokio")]
pub mod rt_tokio;

#[derive(Debug, thiserror::Error)]
#[error("operation timed out")]
pub struct TimeoutError(());

pub enum JoinHandle<T> {
    #[cfg(feature = "_rt-async-std")]
    AsyncStd(async_std::task::JoinHandle<T>),
    #[cfg(feature = "_rt-tokio")]
    Tokio(tokio::task::JoinHandle<T>),
    #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
    Wasm(futures_channel::oneshot::Receiver<T>),
    // `PhantomData<T>` requires `T: Unpin`
    _Phantom(PhantomData<fn() -> T>),
}

pub async fn timeout<F: Future>(duration: Duration, f: F) -> Result<F::Output, TimeoutError> {
    #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
    {
        use futures_util::future::{select, Either};
        let mut user_fut = std::pin::pin!(f);
        let mut timeout_fut = std::pin::pin!(sleep_send(duration));
        return match select(user_fut, timeout_fut).await {
            Either::Left((out, _)) => Ok(out),
            Either::Right((_, _)) => Err(TimeoutError(())),
        };
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "_rt-tokio"))]
    if rt_tokio::available() {
        return tokio::time::timeout(duration, f)
            .await
            .map_err(|_| TimeoutError(()));
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "_rt-async-std"))]
    {
        return async_std::future::timeout(duration, f)
            .await
            .map_err(|_| TimeoutError(()));
    }

    missing_rt((duration, f))
}

/// Send-safe yield: yields once then completes (no gloo/wasm_bindgen).
#[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
async fn yield_once_send() {
    struct YieldOnce(bool);
    impl std::future::Future for YieldOnce {
        type Output = ();
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<()> {
            if self.0 {
                std::task::Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }
    }
    YieldOnce(false).await
}

#[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
async fn sleep_send(duration: Duration) {
    let start = crate::time::Instant::now();
    while start.elapsed() < duration {
        yield_once_send().await;
    }
}

pub async fn sleep(duration: Duration) {
    #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
    {
        sleep_send(duration).await
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "_rt-tokio"))]
    if rt_tokio::available() {
        return tokio::time::sleep(duration).await;
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "_rt-async-std"))]
    {
        async_std::task::sleep(duration).await
    }

    #[cfg(not(any(feature = "_rt-async-std", feature = "_rt-tokio")))]
    missing_rt(duration)
}

#[cfg(target_arch = "wasm32")]
#[track_caller]
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: Send + 'static,
{
    #[cfg(feature = "_rt-async-std")]
    {
        let (tx, rx) = futures_channel::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(fut.await);
        });
        return JoinHandle::Wasm(rx);
    }
    #[cfg(not(feature = "_rt-async-std"))]
    missing_rt(fut)
}

#[cfg(not(target_arch = "wasm32"))]
#[track_caller]
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    #[cfg(feature = "_rt-tokio")]
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return JoinHandle::Tokio(handle.spawn(fut));
    }

    #[cfg(feature = "_rt-async-std")]
    {
        JoinHandle::AsyncStd(async_std::task::spawn(fut))
    }

    #[cfg(not(any(feature = "_rt-async-std", feature = "_rt-tokio")))]
    missing_rt(fut)
}

#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
    {
        let (tx, rx) = futures_channel::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(f());
        });
        return JoinHandle::Wasm(rx);
    }

    #[cfg(feature = "_rt-tokio")]
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return JoinHandle::Tokio(handle.spawn_blocking(f));
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "_rt-async-std"))]
    {
        JoinHandle::AsyncStd(async_std::task::spawn_blocking(f))
    }

    #[cfg(not(any(feature = "_rt-async-std", feature = "_rt-tokio")))]
    missing_rt(f)
}

pub async fn yield_now() {
    #[cfg(feature = "_rt-tokio")]
    if rt_tokio::available() {
        return tokio::task::yield_now().await;
    }

    #[cfg(feature = "_rt-async-std")]
    {
        async_std::task::yield_now().await;
    }

    #[cfg(not(feature = "_rt-async-std"))]
    missing_rt(())
}

#[track_caller]
pub fn test_block_on<F: Future>(f: F) -> F::Output {
    #[cfg(feature = "_rt-tokio")]
    {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to start Tokio runtime")
            .block_on(f);
    }

    #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
    {
        panic!("test_block_on is not supported on wasm32")
    }

    #[cfg(all(
        feature = "_rt-async-std",
        not(feature = "_rt-tokio"),
        not(target_arch = "wasm32")
    ))]
    {
        async_std::task::block_on(f)
    }

    #[cfg(not(any(feature = "_rt-async-std", feature = "_rt-tokio")))]
    {
        missing_rt(f)
    }
}

#[track_caller]
pub fn missing_rt<T>(_unused: T) -> ! {
    if cfg!(feature = "_rt-tokio") {
        panic!("this functionality requires a Tokio context")
    }

    panic!("either the `runtime-async-std` or `runtime-tokio` feature must be enabled")
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = T;

    #[track_caller]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut *self {
            #[cfg(feature = "_rt-async-std")]
            Self::AsyncStd(handle) => Pin::new(handle).poll(cx),
            #[cfg(feature = "_rt-tokio")]
            Self::Tokio(handle) => Pin::new(handle)
                .poll(cx)
                .map(|res| res.expect("spawned task panicked")),
            #[cfg(all(target_arch = "wasm32", feature = "_rt-async-std"))]
            Self::Wasm(rx) => Pin::new(rx)
                .poll(cx)
                .map(|r| r.unwrap_or_else(|e| panic!("task join failed: {:?}", e))),
            Self::_Phantom(_) => {
                let _ = cx;
                unreachable!("runtime should have been checked on spawn")
            }
        }
    }
}
