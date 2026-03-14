use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

/// Clock implementation backed by [`async_io`], used when the `async-io` feature is enabled.
#[derive(Debug, Default, Clone)]
pub struct Clock;

pin_project! {
    /// Wraps an [`async_io::Timer`] to adapt its `Future<Output = Instant>` into
    /// `Future<Output = ()>`, as required by the [`Clock::SleepFuture`](crate::Clock::SleepFuture)
    /// associated type.
    pub struct WrappedTimer {
        #[pin]
        timer: ::async_io::Timer,
    }
}

/// Polls the inner [`async_io::Timer`] and discards the `Instant` output on completion.
impl Future for WrappedTimer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.timer.poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Delegates to the [`async_io`] runtime for time operations.
impl crate::Clock for Clock {
    type Instant = std::time::Instant;
    type SleepFuture = WrappedTimer;
    fn now(&self) -> Self::Instant {
        std::time::Instant::now()
    }
    fn sleep(&self, duration: std::time::Duration) -> Self::SleepFuture {
        WrappedTimer {
            timer: ::async_io::Timer::after(duration),
        }
    }
}

/// Provides the [`InstantExt`](crate::InstantExt) implementation for [`std::time::Instant`].
impl crate::InstantExt for std::time::Instant {
    fn now() -> Self {
        std::time::Instant::now()
    }
    fn saturating_duration_since(&self, earlier: Self) -> std::time::Duration {
        self.saturating_duration_since(earlier)
    }
}
