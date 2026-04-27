//! Runtime `spawn`, `sleep`, and `timeout` shims backed by `tokio`.
//!
//! `Elapsed` is the unified timeout error.

mod tokio_rt;
pub(crate) use tokio_rt::*;

/// Wraps a `tokio::task::JoinHandle` so the spawned task is aborted when the
/// wrapper is dropped. Use this for any background task whose lifetime should
/// be tied to a parent handle (e.g. `Page`, `NetworkManager`) to prevent leaks
/// when the parent is dropped without an explicit shutdown.
///
/// This type is returned from public methods that previously returned bare
/// `JoinHandle`s (e.g. [`Page::route`](crate::Page::route), [`Page::expose_function`](crate::Page::expose_function),
/// [`Page::authenticate`](crate::Page::authenticate)); dropping the value cancels the spawned listener,
/// or call [`abort`](Self::abort) explicitly.
#[derive(Debug)]
pub struct AbortOnDrop<T = ()>(pub(crate) tokio::task::JoinHandle<T>);

#[allow(dead_code)] // public API consumed downstream / by examples
impl<T> AbortOnDrop<T> {
    /// Construct from a raw `JoinHandle`.
    pub fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self(handle)
    }

    /// Abort the underlying task immediately.
    ///
    /// Equivalent to dropping the wrapper, but explicit at the call site.
    pub fn abort(self) {
        // The Drop impl below will fire when `self` goes out of scope.
        drop(self);
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deadline has elapsed")
    }
}
impl std::error::Error for Elapsed {}
