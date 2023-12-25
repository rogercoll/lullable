use core::fmt;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use futures::task::AtomicWaker;
use futures_core::future::Future;
use futures_core::task::{Context, Poll};
use futures_core::Stream;
use pin_project_lite::pin_project;
use std::sync::Arc;

pin_project! {
    /// A future/stream which can be remotely short-circuited using an `LullHandle`.
    #[derive(Debug, Clone)]
    #[must_use = "futures/streams do nothing unless you poll them"]
    pub struct Lullable<T> {
        #[pin]
        task: T,
        inner: Arc<LullInner>,
    }
}

impl<T> Lullable<T> {
    /// Creates a new `Lullable` future/stream using an existing `LullRegistration`.
    /// `LullRegistration`s can be acquired through `LullHandle::new`.
    ///
    /// When `abort` is called on the handle tied to `reg` or if `abort` has
    /// already been called, the future/stream will complete immediately without making
    /// any further progress.
    ///
    /// # Examples:
    ///
    /// Usage with futures:
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// use futures::future::{Lullable, LullHandle, Lulled};
    ///
    /// let (abort_handle, abort_registration) = LullHandle::new_pair();
    /// let future = Lullable::new(async { 2 }, abort_registration);
    /// abort_handle.abort();
    /// assert_eq!(future.await, Err(Lulled));
    /// # });
    /// ```
    ///
    /// Usage with streams:
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// # use futures::future::{Lullable, LullHandle};
    /// # use futures::stream::{self, StreamExt};
    ///
    /// let (abort_handle, abort_registration) = LullHandle::new_pair();
    /// let mut stream = Lullable::new(stream::iter(vec![1, 2, 3]), abort_registration);
    /// abort_handle.abort();
    /// assert_eq!(stream.next().await, None);
    /// # });
    /// ```
    ///
    /// When `pause` is called on the handle tied to `reg` or if `pause` has
    /// already been called, the future/stream will be stopped from being polled.
    ///
    pub fn new(task: T, reg: LullRegistration) -> Self {
        Self {
            task,
            inner: reg.inner,
        }
    }

    /// Checks whether the task has been aborted. Note that all this
    /// method indicates is whether [`LullHandle::abort`] was *called*.
    /// This means that it will return `true` even if:
    /// * `abort` was called after the task had completed.
    /// * `abort` was called while the task was being polled - the task may still be running and
    /// will not be stopped until `poll` returns.
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Relaxed)
    }

    /// Checks whether the task has been paused. Note that all this
    /// method indicates is whether [`LullHandle::pause`] was *called*.
    /// This means that it will return `true` even if:
    /// * `pause` was called after the task had completed.
    /// * `pause` was called while the task was being polled - the task may still be running and
    /// will not be stopped until `poll` returns.
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Relaxed)
    }
}

/// A registration handle for an `Lullable` task.
/// Values of this type can be acquired from `LullHandle::new` and are used
/// in calls to `Lullable::new`.
#[derive(Debug)]
pub struct LullRegistration {
    pub(crate) inner: Arc<LullInner>,
}

impl LullRegistration {
    /// Create an [`LullHandle`] from the given [`LullRegistration`].
    ///
    /// The created [`LullHandle`] is functionally the same as any other
    /// [`LullHandle`]s that are associated with the same [`LullRegistration`],
    /// such as the one created by [`LullHandle::new_pair`].
    pub fn handle(&self) -> LullHandle {
        LullHandle {
            inner: self.inner.clone(),
        }
    }
}

/// A handle to an `Lullable` task.
#[derive(Debug, Clone)]
pub struct LullHandle {
    inner: Arc<LullInner>,
}

impl LullHandle {
    /// Creates an (`LullHandle`, `LullRegistration`) pair which can be used
    /// to abort a running future or stream.
    ///
    /// This function is usually paired with a call to [`Lullable::new`].
    pub fn new_pair() -> (Self, LullRegistration) {
        let inner = Arc::new(LullInner {
            waker: AtomicWaker::new(),
            aborted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        });

        (
            Self {
                inner: inner.clone(),
            },
            LullRegistration { inner },
        )
    }
}

// Inner type storing the waker to awaken and a bool indicating that it
// should be aborted.
#[derive(Debug)]
pub(crate) struct LullInner {
    pub(crate) waker: AtomicWaker,
    pub(crate) aborted: AtomicBool,
    pub(crate) paused: AtomicBool,
}

/// Indicator that the `Lullable` task was aborted.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Aborted;

impl fmt::Display for Aborted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`Lullable` future has been aborted")
    }
}

impl<T> Lullable<T> {
    fn try_poll<I>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        poll: impl Fn(Pin<&mut T>, &mut Context<'_>) -> Poll<I>,
    ) -> Poll<Result<I, Aborted>> {
        // Check if the task has been aborted
        if self.is_aborted() {
            return Poll::Ready(Err(Aborted));
        } else if self.is_paused() {
            // Register to receive a wakeup if the task is aborted in the future
            self.inner.waker.register(cx.waker());
            return Poll::Pending;
        }

        // attempt to complete the task
        if let Poll::Ready(x) = poll(self.as_mut().project().task, cx) {
            return Poll::Ready(Ok(x));
        }

        // Register to receive a wakeup if the task is aborted in the future
        self.inner.waker.register(cx.waker());

        // Check to see if the task was aborted between the first check and
        // registration.
        // Checking with `is_aborted` which uses `Relaxed` is sufficient because
        // `register` introduces an `AcqRel` barrier.
        if self.is_aborted() {
            return Poll::Ready(Err(Aborted));
        }

        Poll::Pending
    }
}

impl<Fut> Future for Lullable<Fut>
where
    Fut: Future,
{
    type Output = Result<Fut::Output, Aborted>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.try_poll(cx, |fut, cx| fut.poll(cx))
    }
}

impl<St> Stream for Lullable<St>
where
    St: Stream,
{
    type Item = St::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.try_poll(cx, |stream, cx| stream.poll_next(cx))
            .map(Result::ok)
            .map(Option::flatten)
    }
}

pub struct PauseGuard<'a> {
    inner: &'a LullHandle,
}

impl<'a> Drop for PauseGuard<'a> {
    fn drop(&mut self) {
        self.inner.unpause();
    }
}

impl LullHandle {
    /// Abort the `Lullable` stream/future associated with this handle.
    ///
    /// Notifies the Lullable task associated with this handle that it
    /// should abort. Note that if the task is currently being polled on
    /// another thread, it will not immediately stop running. Instead, it will
    /// continue to run until its poll method returns.
    pub fn abort(&self) {
        self.inner.aborted.store(true, Ordering::Relaxed);
        self.inner.waker.wake();
    }

    /// Pause the `Lullable` stream/future associated with this handle.
    ///
    /// Notifies the Lullable task associated with this handle that it
    /// should pause the polling. Note that if the task is currently being polled on
    /// another thread, it will not stop until the task is yielded. Instead, it will
    /// continue to run until its poll method returns.
    ///
    /// A PauseGuard is returned, task polling won't be enabled until dropped.
    pub fn pause(&self) -> PauseGuard {
        self.inner.paused.store(true, Ordering::Relaxed);
        self.inner.waker.wake();

        PauseGuard { inner: self }
    }

    fn unpause(&self) {
        self.inner.paused.store(false, Ordering::Relaxed);
        self.inner.waker.wake();
    }

    /// Checks whether [`LullHandle::abort`] was *called* on any associated
    /// [`LullHandle`]s, which includes all the [`LullHandle`]s linked with
    /// the same [`LullRegistration`]. This means that it will return `true`
    /// even if:
    /// * `abort` was called after the task had completed.
    /// * `abort` was called while the task was being polled - the task may still be running and
    /// will not be stopped until `poll` returns.
    ///
    /// This operation has a Relaxed ordering.
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Relaxed)
    }
}

pub fn lullable<Fut>(future: Fut) -> (Lullable<Fut>, LullHandle)
where
    Fut: Future,
{
    let (handle, reg) = LullHandle::new_pair();
    let pausable = Lullable::new(future, reg);
    (pausable, handle)
}
