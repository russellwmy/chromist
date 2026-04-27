use std::future::Future;
use std::time::Duration;

pub(crate) use tokio::task::JoinHandle;

pub(crate) fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(fut)
}

pub(crate) async fn sleep(dur: Duration) {
    tokio::time::sleep(dur).await;
}

pub(crate) async fn timeout<F: Future>(dur: Duration, fut: F) -> Result<F::Output, super::Elapsed> {
    tokio::time::timeout(dur, fut).await.map_err(|_| super::Elapsed)
}
