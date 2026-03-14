//! A hybrid async sleep implementation that combines runtime-based timers with
//! spin-sleeping for precise, low-latency delays.
//!
//! # How it works
//!
//! For durations longer than a configurable threshold (default: 10ms), the sleep
//! delegates to the underlying async runtime's timer (tokio or async-io). Once the
//! remaining time drops below the threshold, it switches to a spin-loop that
//! repeatedly wakes and checks the deadline, trading CPU usage for sub-millisecond
//! accuracy.
//!
//! # Runtime support
//!
//! Enable exactly one runtime feature:
//!
//! - **`tokio`** — uses `tokio::time` (default)
//! - **`async-io`** — uses `async_io::Timer`
//! - **`smol`** — uses `async-io` under the hood, adds `smol` dev-test support
//!
//! # Examples
//!
//! ```rust,no_run
//! # #[cfg(any(
//! #     all(feature = "tokio", not(feature = "async-io")),
//! #     all(feature = "async-io", not(feature = "tokio"))
//! # ))]
//! # async fn example() {
//! use std::time::Duration;
//!
//! // Simple sleep
//! async_hybrid_sleep::sleep(Duration::from_millis(100)).await;
//!
//! // Periodic ticking
//! let mut interval = async_hybrid_sleep::interval(Duration::from_millis(16));
//! loop {
//!     interval.tick().await;
//!     // ~60 Hz tick
//! }
//! # }
//! ```

use std::{future::poll_fn, pin::Pin, task::Poll, time::Duration};

#[cfg(feature = "tokio")]
mod tokio;
#[cfg(feature = "tokio")]
pub type TokioClock = tokio::Clock;

#[cfg(feature = "async-io")]
mod async_io;
#[cfg(feature = "async-io")]
pub type AsyncIoClock = async_io::Clock;

/// Extension trait for instant types used by this crate.
///
/// Abstracts over [`std::time::Instant`] and `tokio::time::Instant` so that
/// [`Sleep`] and [`Interval`] can work with either backend (or a custom
/// implementation for testing).
pub trait InstantExt:
    Copy
    + Ord
    + std::ops::Add<Duration, Output = Self>
    + std::ops::Sub<Duration, Output = Self>
    + Unpin
    + From<std::time::Instant>
    + Into<std::time::Instant>
{
    /// Returns the duration elapsed from `earlier` to `self`, or
    /// [`Duration::ZERO`] if `earlier` is later than `self`.
    fn saturating_duration_since(&self, earlier: Self) -> Duration;
}

/// Trait for pluggable async clock backends.
///
/// A `Clock` knows how to read the current time and how to produce a
/// [`Future`] that resolves after a given [`Duration`].  The crate
/// ships two implementations behind feature flags (`tokio` and
/// `async-io`), but users may supply their own for testing or other
/// runtimes.
pub trait Clock: Clone + Unpin + Sized + Default {
    /// The instant type returned by [`now`](Clock::now).
    type Instant: InstantExt;

    /// The future type returned by [`sleep`](Clock::sleep).
    type SleepFuture: std::future::Future<Output = ()> + Send + 'static;

    /// Returns the current instant according to this clock.
    fn now(&self) -> Self::Instant;

    /// Returns a future that completes after `duration` has elapsed.
    fn sleep(&self, duration: Duration) -> Self::SleepFuture;

    /// Returns the duration elapsed since `self` was recorded.
    ///
    /// Equivalent to `Self::now().saturating_duration_since(*self)`.
    #[cfg(test)]
    fn elapsed(&self, earlier: Self::Instant) -> Duration {
        self.now().saturating_duration_since(earlier)
    }
}

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Default clock and instant types selected by the active runtime feature.
///
/// When the `tokio` feature is enabled the types delegate to
/// `tokio::time`; with `async-io` they delegate to `async_io` and
/// `std::time`.
pub mod default {
    /// The runtime-specific instant type used by the [`default::Clock`](Clock).
    pub type Instant = <Clock as super::Clock>::Instant;

    /// The default [`Clock`](super::Clock) implementation, selected
    /// automatically from the enabled runtime feature (`tokio` or
    /// `async-io`).
    #[derive(Debug, Default, Clone)]
    pub struct Clock {
        #[cfg(feature = "tokio")]
        inner: crate::tokio::Clock,
        #[cfg(feature = "async-io")]
        inner: crate::async_io::Clock,
    }

    /// Delegates to the runtime-specific inner clock.
    impl super::Clock for Clock {
        #[cfg(feature = "tokio")]
        type Instant = ::tokio::time::Instant;
        #[cfg(feature = "tokio")]
        type SleepFuture = ::tokio::time::Sleep;

        #[cfg(feature = "async-io")]
        type Instant = std::time::Instant;
        #[cfg(feature = "async-io")]
        type SleepFuture = crate::async_io::WrappedTimer;

        fn now(&self) -> Self::Instant {
            self.inner.now()
        }

        fn sleep(&self, duration: std::time::Duration) -> Self::SleepFuture {
            self.inner.sleep(duration)
        }
    }
}

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Re-export of [`default::Clock`] for convenient use as a type parameter
/// default.
pub use default::Clock as DefaultClock;

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Re-export of the runtime-specific [`default::Instant`] type.
pub use default::Instant;

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Creates a [`Sleep`] future that completes after the given `duration`.
///
/// This is the primary entry point for one-shot delays. The returned future
/// uses the hybrid strategy: runtime timer for the bulk of the wait, then
/// spin-sleeping for the final stretch.
pub fn sleep(duration: Duration) -> Sleep<DefaultClock> {
    sleep_with_clock(DefaultClock::default(), duration)
}

/// Like `sleep`, but uses a caller-supplied [`Clock`] instead of the
/// `DefaultClock`.
///
/// This is useful when you want to test with a mock clock or use a
/// non-default runtime.
pub fn sleep_with_clock<C: Clock>(clock: C, duration: Duration) -> Sleep<C> {
    let deadline = clock.now() + duration;
    Sleep::new(clock, deadline)
}

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Creates a [`Sleep`] future that completes at the given `deadline`.
///
/// If `deadline` is already in the past the future will resolve immediately
/// on its first poll.
pub fn sleep_until(deadline: std::time::Instant) -> Sleep<DefaultClock> {
    #[allow(clippy::useless_conversion)]
    sleep_until_with_clock(DefaultClock::default(), deadline.into())
}

/// Like `sleep_until`, but uses a caller-supplied [`Clock`] instead of the
/// `DefaultClock`.
pub fn sleep_until_with_clock<C: Clock>(clock: C, deadline: C::Instant) -> Sleep<C> {
    Sleep::new(clock, deadline)
}

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Creates an [`Interval`] that yields repeatedly every `period`.
///
/// The first tick completes after one full `period` has elapsed.
pub fn interval(period: Duration) -> Interval<DefaultClock> {
    interval_with_clock(DefaultClock::default(), period)
}

#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
/// Creates a [`SleepBuilder`] targeting the given `deadline` with the
/// default runtime clock.
///
/// This is a convenience wrapper around [`SleepBuilder::new`] that avoids
/// the need for a turbofish type annotation.
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// # async fn example() {
/// let deadline = std::time::Instant::now() + Duration::from_millis(200);
/// async_hybrid_sleep::sleep_builder(deadline)
///     .threshold(Duration::from_millis(5))
///     .build()
///     .await;
/// # }
/// ```
pub fn sleep_builder(deadline: std::time::Instant) -> SleepBuilder {
    SleepBuilder::new(deadline)
}

/// Like `interval`, but uses a caller-supplied [`Clock`] instead of the
/// `DefaultClock`.
pub fn interval_with_clock<C: Clock>(clock: C, period: Duration) -> Interval<C> {
    Interval::new(clock, period)
}

/// A future that resolves at a specific deadline using a hybrid strategy.
///
/// While the remaining time exceeds the configured threshold
/// (`DEFAULT_SLEEP_THRESHOLD`),
/// the future delegates to the async runtime's native timer. Once the remaining
/// time is below the threshold it spin-sleeps — waking the task on every poll —
/// to achieve sub-millisecond precision.
///
/// Use `sleep`, `sleep_until`, or [`SleepBuilder`] to construct a `Sleep`.
#[derive(Debug)]
pub struct Sleep<C: Clock> {
    clock: C,
    deadline: C::Instant,
    threshold: Duration,
    inner_sleep: Option<Pin<Box<C::SleepFuture>>>,
    #[cfg(all(feature = "test-util", feature = "tokio"))]
    start_paused: bool,
    #[cfg(all(feature = "test-util", feature = "tokio"))]
    test_util_inner_sleep: Option<Pin<Box<::tokio::time::Sleep>>>,
}

impl<C: Clock> Sleep<C> {
    /// The default threshold below which the future switches from a runtime
    /// timer to spin-sleeping (10 ms).
    pub const DEFAULT_SLEEP_THRESHOLD: Duration = Duration::from_millis(10);

    /// Creates a new `Sleep` that will complete at `deadline` using the
    /// default threshold.
    pub fn new(clock: C, deadline: C::Instant) -> Self {
        Self {
            clock,
            deadline,
            threshold: Self::DEFAULT_SLEEP_THRESHOLD,
            inner_sleep: None,
            #[cfg(all(feature = "test-util", feature = "tokio"))]
            start_paused: false,
            #[cfg(all(feature = "test-util", feature = "tokio"))]
            test_util_inner_sleep: None,
        }
    }

    /// Resets this sleep to a new deadline.
    ///
    /// Any in-progress runtime timer is dropped so the next poll will
    /// re-evaluate the remaining duration from scratch.
    pub fn reset(self: Pin<&mut Self>, new_deadline: C::Instant) {
        let this = self.get_mut();
        this.deadline = new_deadline;
        this.inner_sleep = None; // Drop the existing sleep future, if any, to reset it
        #[cfg(all(feature = "test-util", feature = "tokio"))]
        {
            this.test_util_inner_sleep = None;
        }
    }

    fn clock(&self) -> &C {
        &self.clock
    }
}

/// Builder for constructing a [`Sleep`] future with custom settings.
///
/// # Examples
///
/// ```rust,no_run
/// use std::time::Duration;
///
/// # async fn example() {
/// let deadline = async_hybrid_sleep::Instant::now() + Duration::from_millis(200);
/// let sleep: async_hybrid_sleep::Sleep<async_hybrid_sleep::DefaultClock> =
///     async_hybrid_sleep::SleepBuilder::new(deadline.into())
///         .threshold(Duration::from_millis(5))
///         .build();
/// # }
/// ```
#[cfg(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
))]
pub struct SleepBuilder {
    inner: inner::SleepBuilder<DefaultClock>,
}

/// Builder for constructing a [`Sleep`] future with custom settings.
///
/// This variant is used when no default runtime feature is enabled (i.e.
/// neither `tokio` nor `async-io`). A clock type must be supplied
/// explicitly.
#[cfg(not(any(
    all(feature = "tokio", not(feature = "async-io")),
    all(feature = "async-io", not(feature = "tokio"))
)))]
pub struct SleepBuilder<C: Clock> {
    inner: inner::SleepBuilder<C>,
}

mod inner {
    use std::time::Duration;

    pub(crate) struct SleepBuilder<C: super::Clock> {
        pub(crate) deadline: std::time::Instant,
        pub(crate) threshold: Option<Duration>,
        pub(crate) clock: Option<C>,
        #[cfg(all(feature = "test-util", feature = "tokio"))]
        pub(crate) start_paused: bool,
    }
}

/// Generates the method bodies for `SleepBuilder`, parameterised by a
/// clock type `$C`.
///
/// This is a helper invoked by [`builder_impl!`] — it exists so that the
/// token `$C` is an actual macro metavariable that gets substituted into
/// every position (return types, turbofish, etc.) before the compiler ever
/// sees the tokens.
macro_rules! builder_methods {
    ($C:ty) => {
        /// Creates a new builder targeting the given `deadline`.
        pub fn new(deadline: std::time::Instant) -> Self {
            Self {
                inner: inner::SleepBuilder {
                    deadline,
                    threshold: None,
                    clock: None,
                    #[cfg(all(feature = "test-util", feature = "tokio"))]
                    start_paused: false,
                },
            }
        }

        /// Sets the spin-sleep threshold.
        ///
        /// When the remaining time until the deadline is less than `threshold`,
        /// the future will stop using the runtime timer and instead spin-sleep
        /// for maximum precision.  Smaller values reduce CPU usage at the cost
        /// of timing accuracy.
        pub fn threshold(&mut self, threshold: Duration) -> &mut Self {
            self.inner.threshold = Some(threshold);
            self
        }

        /// Indicates that this sleep will run inside a Tokio test with
        /// `start_paused = true`.
        ///
        /// When enabled the spin-sleep phase is replaced with a real
        /// [`::tokio::time::sleep`] so that Tokio's auto-advance mechanism can
        /// move time forward correctly.
        #[cfg(all(feature = "test-util", feature = "tokio"))]
        pub fn start_paused(&mut self, start_paused: bool) -> &mut Self {
            self.inner.start_paused = start_paused;
            self
        }

        /// Consumes the builder and returns the configured [`Sleep`] future.
        pub fn build(&self) -> Sleep<$C> {
            let clock: $C = self.inner.clock.clone().unwrap_or_default();
            Sleep {
                deadline: self.inner.deadline.into(),
                threshold: self
                    .inner
                    .threshold
                    .unwrap_or(Sleep::<$C>::DEFAULT_SLEEP_THRESHOLD),
                clock,
                inner_sleep: None,
                #[cfg(all(feature = "test-util", feature = "tokio"))]
                start_paused: self.inner.start_paused,
                #[cfg(all(feature = "test-util", feature = "tokio"))]
                test_util_inner_sleep: None,
            }
        }
    };
}

/// Emits `impl` blocks for both `SleepBuilder` variants from a single set
/// of method definitions, avoiding near-identical cfg-gated duplicates.
///
/// When exactly one of `tokio` / `async-io` is enabled, `DefaultClock`
/// exists and `SleepBuilder` is a concrete (non-generic) struct.  The
/// macro emits `impl SleepBuilder` with `DefaultClock` substituted for the
/// clock type.
///
/// When *neither* or *both* runtime features are enabled there is no
/// `DefaultClock`, so `SleepBuilder<C: Clock>` is generic.  The macro
/// emits `impl<C: Clock> SleepBuilder<C>` with `C` as the clock type.
macro_rules! builder_impl {
    () => {
        #[cfg(any(
            all(feature = "tokio", not(feature = "async-io")),
            all(feature = "async-io", not(feature = "tokio"))
        ))]
        impl SleepBuilder {
            builder_methods!(DefaultClock);
        }

        #[cfg(not(any(
            all(feature = "tokio", not(feature = "async-io")),
            all(feature = "async-io", not(feature = "tokio"))
        )))]
        impl<C: Clock> SleepBuilder<C> {
            builder_methods!(C);
        }
    };
}

builder_impl!();

impl<C: Clock> Future for Sleep<C> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let remaining = self.deadline.saturating_duration_since(self.clock.now());
        if remaining.is_zero() {
            return Poll::Ready(());
        }

        if remaining > self.threshold {
            if self.inner_sleep.is_none() {
                self.inner_sleep = Some(Box::pin(self.clock.sleep(remaining - self.threshold)));
            }
            match self.inner_sleep.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Ready(_) => {
                    self.inner_sleep = None;
                    return Poll::Pending; // Continue to check the deadline
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        #[cfg(all(feature = "test-util", feature = "tokio"))]
        {
            if self.start_paused {
                // When tokio::test is used with start_paused, time won't advance.
                // The only way for it to auto-advance is to poll the tokio's sleep future.
                // As we are here as a sleep future, we need to make the time advance by
                // by a remaining duration of the sleep, which is the time until the deadline.
                if self.test_util_inner_sleep.is_none() {
                    self.test_util_inner_sleep = Some(Box::pin(::tokio::time::sleep(remaining)));
                }
                return match self
                    .test_util_inner_sleep
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .poll(cx)
                {
                    Poll::Ready(_) => Poll::Ready(()),
                    Poll::Pending => Poll::Pending,
                };
            }
        }
        // Wake the task to check the deadline again - spin-sleeping for the remaining short duration
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// A stream-like handle that yields at a fixed period using [`Sleep`] under
/// the hood.
///
/// Created with `interval`. Each call to [`tick`](Interval::tick) waits
/// until the next deadline, then resets to `now + period`.
pub struct Interval<C: Clock> {
    delay: Pin<Box<Sleep<C>>>,
    period: Duration,
}

impl<C: Clock> Interval<C> {
    /// Creates a new `Interval` that fires every `period`.
    ///
    /// The first tick completes `period` after construction.
    pub fn new(clock: C, period: Duration) -> Self {
        let deadline = clock.now() + period;
        Self {
            delay: Box::pin(Sleep::new(clock, deadline)),
            period,
        }
    }

    /// Resets the interval so the next tick fires one full `period` from now.
    pub fn reset(&mut self) {
        self.reset_inner(self.delay.clock().now() + self.period);
    }

    /// Resets the interval so the very next poll completes immediately.
    pub fn reset_immediately(&mut self) {
        self.reset_inner(self.delay.clock().now());
    }

    fn reset_inner(&mut self, deadline: C::Instant) {
        self.delay.as_mut().reset(deadline);
    }

    /// Waits until the next tick deadline is reached.
    pub async fn tick(&mut self) {
        poll_fn(|cx| self.poll_tick(cx)).await
    }

    /// Polls the interval, returning `Poll::Ready(())` when the current
    /// deadline has elapsed and automatically scheduling the next tick.
    pub fn poll_tick(&mut self, cx: &mut std::task::Context<'_>) -> Poll<()> {
        match self.delay.as_mut().poll(cx) {
            Poll::Ready(_) => {
                self.reset(); // Schedule the next tick
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) async fn test_sleep<C: Clock>(clock: C) {
        let start = clock.now();
        super::sleep_with_clock(clock.clone(), Duration::from_millis(50)).await;
        assert!(clock.elapsed(start) >= Duration::from_millis(50));
    }

    pub(crate) async fn test_sleep_until<C: Clock>(clock: C) {
        let start = clock.now();
        let deadline = start + Duration::from_millis(50);
        super::sleep_until_with_clock(clock.clone(), deadline).await;
        assert!(clock.elapsed(start) >= Duration::from_millis(50));
    }

    pub(crate) async fn test_sleep_already_passed<C: Clock>(clock: C) {
        let start = clock.now();
        let deadline = start - Duration::from_millis(10);
        super::sleep_until_with_clock(clock.clone(), deadline).await;
        assert!(clock.elapsed(start) < Duration::from_millis(50));
    }

    pub(crate) async fn test_sleep_short_duration<C: Clock>(clock: C) {
        let start = clock.now();
        let deadline = start + Duration::from_millis(5);
        super::sleep_until_with_clock(clock.clone(), deadline).await;
        assert!(clock.elapsed(start) >= Duration::from_millis(5));
    }

    pub(crate) async fn test_sleep_long_duration<C: Clock>(clock: C) {
        let start = clock.now();
        let deadline = start + Duration::from_millis(100);
        super::sleep_until_with_clock(clock.clone(), deadline).await;
        assert!(clock.elapsed(start) >= Duration::from_millis(100));
    }

    pub(crate) async fn test_interval_basic<C: Clock>(clock: C) {
        let period = Duration::from_millis(50);
        let start = clock.now();
        let mut interval = super::interval_with_clock(clock.clone(), period);
        interval.tick().await;
        assert!(clock.elapsed(start) >= period);
        interval.tick().await;
        assert!(clock.elapsed(start) >= period * 2);
    }

    pub(crate) async fn test_interval_reset<C: Clock>(clock: C) {
        let period = Duration::from_millis(50);
        let mut interval = super::interval_with_clock(clock.clone(), period);
        // Wait for the first tick
        interval.tick().await;
        let before_reset = clock.now();
        // Reset pushes the next tick one full period from now
        interval.reset();
        interval.tick().await;
        assert!(clock.elapsed(before_reset) >= period);
    }

    pub(crate) async fn test_interval_reset_immediately<C: Clock>(clock: C) {
        let period = Duration::from_millis(100);
        let mut interval = super::interval_with_clock(clock.clone(), period);
        // Wait for the first tick
        interval.tick().await;
        let before_reset = clock.now();
        // Reset immediately should resolve on the next poll
        interval.reset_immediately();
        interval.tick().await;
        // Should complete almost instantly (well under one period)
        assert!(clock.elapsed(before_reset) < period);
    }

    pub(crate) async fn test_interval_multiple_ticks<C: Clock>(clock: C) {
        let period = Duration::from_millis(30);
        let start = clock.now();
        let mut interval = super::interval_with_clock(clock.clone(), period);
        for i in 1..=4u32 {
            interval.tick().await;
            assert!(clock.elapsed(start) >= period * i);
        }
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tokio_tests {
    use super::tokio::Clock as TokioClock;
    use super::{Duration, SleepBuilder};

    #[::tokio::test]
    async fn test_sleep() {
        super::tests::test_sleep(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_sleep_until() {
        super::tests::test_sleep_until(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_sleep_already_passed() {
        super::tests::test_sleep_already_passed(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_sleep_short_duration() {
        super::tests::test_sleep_short_duration(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_sleep_long_duration() {
        super::tests::test_sleep_long_duration(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_interval_basic() {
        super::tests::test_interval_basic(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_interval_reset() {
        super::tests::test_interval_reset(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_interval_reset_immediately() {
        super::tests::test_interval_reset_immediately(TokioClock::new()).await;
    }

    #[::tokio::test]
    async fn test_interval_multiple_ticks() {
        super::tests::test_interval_multiple_ticks(TokioClock::new()).await;
    }

    #[::tokio::test(start_paused = true)]
    #[cfg(feature = "test-util")]
    async fn test_sleep_with_time_advance() {
        use ::tokio::time::Instant;
        let start = Instant::now();
        let deadline = start + Duration::from_millis(50);
        let sleep_future = SleepBuilder::new(deadline.into())
            .start_paused(true)
            .build();
        ::tokio::time::advance(Duration::from_millis(30)).await;
        assert!(start.elapsed() >= Duration::from_millis(30));
        sleep_future.await;
        assert!(start.elapsed() >= Duration::from_millis(50));
    }

    #[::tokio::test]
    async fn test_sleep_move_to_task() {
        use ::tokio::time::Instant;
        let start = Instant::now();
        let deadline = start + Duration::from_millis(50);
        #[cfg(all(feature = "tokio", not(feature = "async-io")))]
        let sleep_future = super::sleep_until(deadline.into());
        #[cfg(all(feature = "tokio", feature = "async-io"))]
        let sleep_future = super::sleep_until_with_clock(super::TokioClock::new(), deadline);
        ::tokio::spawn(async move {
            sleep_future.await;
        })
        .await
        .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(50));
    }

    #[::tokio::test]
    async fn test_sleep_reset() {
        use ::tokio::time::Instant;
        let start = Instant::now();
        let deadline = start + Duration::from_millis(50);
        #[cfg(all(feature = "tokio", not(feature = "async-io")))]
        let mut sleep_future = super::sleep_until(deadline.into());
        #[cfg(all(feature = "tokio", feature = "async-io"))]
        let mut sleep_future = super::sleep_until_with_clock(super::TokioClock::new(), deadline);
        let mut _completed = false;
        let mut reset_done = false;
        loop {
            ::tokio::select! {
                _ = &mut sleep_future => {
                    assert!(start.elapsed() >= Duration::from_millis(100));
                    _completed = true;
                    break;
                }
                _ = ::tokio::time::sleep(Duration::from_millis(30)), if !reset_done => {
                    let new_deadline = Instant::now() + Duration::from_millis(70);
                    Pin::new(&mut sleep_future).reset(new_deadline);
                    reset_done = true;
                }
            }
        }
        assert!(_completed);

        #[cfg(all(feature = "tokio", not(feature = "async-io")))]
        let mut sleep_future = SleepBuilder::new((start + Duration::from_millis(50)).into())
            .threshold(Duration::from_millis(30))
            .build();
        #[cfg(all(feature = "tokio", feature = "async-io"))]
        let mut sleep_future =
            SleepBuilder::<TokioClock>::new((start + Duration::from_millis(50)).into())
                .threshold(Duration::from_millis(30))
                .build();
        let mut _completed = false;
        let mut reset_done = false;
        loop {
            ::tokio::select! {
                _ = &mut sleep_future => {
                    assert!(start.elapsed() >= Duration::from_millis(100));
                    _completed = true;
                    break;
                }
                _ = ::tokio::time::sleep(Duration::from_millis(30)), if !reset_done => {
                    let new_deadline = Instant::now() + Duration::from_millis(70);
                    Pin::new(&mut sleep_future).reset(new_deadline);
                    reset_done = true;
                }
            }
        }
        assert!(_completed);
    }
}

#[cfg(all(test, feature = "smol"))]
mod smol_tests {
    smol_macros::test! {
        async fn test_sleep(_ex: &smol::Executor<'_>) {
            super::tests::test_sleep(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_sleep_until(_ex: &smol::Executor<'_>) {
            super::tests::test_sleep_until(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_sleep_already_passed(_ex: &smol::Executor<'_>) {
            super::tests::test_sleep_already_passed(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_sleep_short_duration(_ex: &smol::Executor<'_>) {
            super::tests::test_sleep_short_duration(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_sleep_long_duration(_ex: &smol::Executor<'_>) {
            super::tests::test_sleep_long_duration(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_interval_basic(_ex: &smol::Executor<'_>) {
            super::tests::test_interval_basic(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_interval_reset(_ex: &smol::Executor<'_>) {
            super::tests::test_interval_reset(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_interval_reset_immediately(_ex: &smol::Executor<'_>) {
            super::tests::test_interval_reset_immediately(crate::async_io::Clock).await;
        }
    }

    smol_macros::test! {
        async fn test_interval_multiple_ticks(_ex: &smol::Executor<'_>) {
            super::tests::test_interval_multiple_ticks(crate::async_io::Clock).await;
        }
    }
}

#[cfg(test)]
mod mock_tests {
    use super::*;
    use std::ops::{Add, Sub};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // ── MockInstant ──────────────────────────────────────────────────

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct MockInstant {
        now: std::time::Instant,
    }

    impl MockInstant {
        fn now() -> Self {
            Self {
                now: std::time::Instant::now(),
            }
        }
    }

    impl Default for MockInstant {
        fn default() -> Self {
            Self::now()
        }
    }

    impl Add<std::time::Duration> for MockInstant {
        type Output = Self;

        fn add(self, rhs: std::time::Duration) -> Self {
            Self {
                now: self.now + rhs,
            }
        }
    }

    impl Sub<std::time::Duration> for MockInstant {
        type Output = Self;

        fn sub(self, rhs: std::time::Duration) -> Self {
            Self {
                now: self.now - rhs,
            }
        }
    }

    impl From<std::time::Instant> for MockInstant {
        fn from(now: std::time::Instant) -> Self {
            Self { now }
        }
    }

    impl From<MockInstant> for std::time::Instant {
        fn from(mock: MockInstant) -> Self {
            mock.now
        }
    }

    impl InstantExt for MockInstant {
        // fn now() -> Self {
        //     Self::new()
        // }

        fn saturating_duration_since(&self, earlier: Self) -> Duration {
            self.now.saturating_duration_since(earlier.now)
        }
    }

    // ── MockSleepFuture ──────────────────────────────────────────────

    /// A `Send + 'static` future that records a wall-clock deadline on
    /// first poll, then returns `Pending` until that deadline has passed.
    /// Uses `std::thread::sleep` for the bulk wait and spin-checks for
    /// precision — no async runtime needed.
    struct MockSleepFuture {
        duration: Duration,
        deadline: Option<std::time::Instant>,
    }

    impl std::future::Future for MockSleepFuture {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let dur = self.duration;
            let deadline = *self
                .deadline
                .get_or_insert_with(|| std::time::Instant::now() + dur);

            if std::time::Instant::now() >= deadline {
                return Poll::Ready(());
            }

            // Block for most of the remaining time, leaving a small margin
            // for the caller to spin-check precisely.
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining > Duration::from_millis(1) {
                std::thread::sleep(remaining - Duration::from_millis(1));
            }

            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    // ── MockClock ────────────────────────────────────────────────────

    /// A `Clock` implementation backed entirely by `std::time` and
    /// `std::thread::sleep` – no tokio / async-io required.
    #[derive(Debug, Clone, Default)]
    struct MockClock;

    impl Clock for MockClock {
        type Instant = MockInstant;
        type SleepFuture = MockSleepFuture;

        fn now(&self) -> MockInstant {
            MockInstant::now()
        }

        fn sleep(&self, duration: Duration) -> MockSleepFuture {
            MockSleepFuture {
                duration,
                deadline: None,
            }
        }
    }

    // ── Minimal single-threaded executor ─────────────────────────────

    /// Drive a future to completion on the current thread.
    fn block_on<F: std::future::Future<Output = ()>>(fut: F) {
        use std::sync::Arc;
        use std::task::{Wake, Waker};

        struct NoopWaker;
        impl Wake for NoopWaker {
            fn wake(self: Arc<Self>) {}
        }

        let waker: Waker = Arc::new(NoopWaker).into();
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);

        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(()) => return,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[test]
    fn test_sleep() {
        block_on(super::tests::test_sleep(MockClock));
    }

    #[test]
    fn test_sleep_until() {
        block_on(super::tests::test_sleep_until(MockClock));
    }

    #[test]
    fn test_sleep_already_passed() {
        block_on(super::tests::test_sleep_already_passed(MockClock));
    }

    #[test]
    fn test_sleep_short_duration() {
        block_on(super::tests::test_sleep_short_duration(MockClock));
    }

    #[test]
    fn test_sleep_long_duration() {
        block_on(super::tests::test_sleep_long_duration(MockClock));
    }

    #[test]
    fn test_interval_basic() {
        block_on(super::tests::test_interval_basic(MockClock));
    }

    #[test]
    fn test_interval_reset() {
        block_on(super::tests::test_interval_reset(MockClock));
    }

    #[test]
    fn test_interval_reset_immediately() {
        block_on(super::tests::test_interval_reset_immediately(MockClock));
    }

    #[test]
    fn test_interval_multiple_ticks() {
        block_on(super::tests::test_interval_multiple_ticks(MockClock));
    }

    /// Tests SleepBuilder with a custom clock.
    ///
    /// When exactly one runtime feature is enabled `SleepBuilder` is
    /// non-generic (tied to `DefaultClock`), so we exercise it with
    /// the default clock in that configuration.  When the builder IS
    /// generic we test it with `MockClock`.
    #[test]
    #[cfg(any(
        all(feature = "tokio", not(feature = "async-io")),
        all(feature = "async-io", not(feature = "tokio"))
    ))]
    fn test_sleep_builder_with_mock_clock() {
        block_on(async {
            let clock = MockClock;
            let start = clock.now();
            // Use sleep_with_clock to test the mock clock path with a
            // custom-ish duration; the builder is not generic here.
            let sleep = sleep_with_clock(clock.clone(), Duration::from_millis(50));
            sleep.await;
            assert!(clock.elapsed(start) >= Duration::from_millis(50));
        });
    }

    #[test]
    #[cfg(not(any(
        all(feature = "tokio", not(feature = "async-io")),
        all(feature = "async-io", not(feature = "tokio"))
    )))]
    fn test_sleep_builder_with_mock_clock() {
        block_on(async {
            let clock = MockClock;
            let start = clock.now();
            let deadline = start + Duration::from_millis(50);
            let sleep = SleepBuilder::<MockClock>::new(deadline.into())
                .threshold(Duration::from_millis(5))
                .build();
            sleep.await;
            assert!(clock.elapsed(start) >= Duration::from_millis(50));
        });
    }

    #[test]
    fn test_sleep_reset_with_mock_clock() {
        block_on(async {
            let clock = MockClock;
            let start = clock.now();
            let deadline = start + Duration::from_millis(30);
            let mut sleep = sleep_until_with_clock(clock.clone(), deadline);
            // Immediately reset to a later deadline before it fires
            let new_deadline = start + Duration::from_millis(60);
            Pin::new(&mut sleep).reset(new_deadline);
            sleep.await;
            assert!(clock.elapsed(start) >= Duration::from_millis(60));
        });
    }

    #[test]
    fn test_sleep_zero_duration() {
        block_on(async {
            let clock = MockClock;
            let start = clock.now();
            sleep_with_clock(clock.clone(), Duration::ZERO).await;
            // Should complete almost instantly
            assert!(clock.elapsed(start) < Duration::from_millis(50));
        });
    }
}
