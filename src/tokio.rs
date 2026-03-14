/// Tokio-backed clock implementation, used when the `tokio` feature is enabled.
#[derive(Debug, Clone, Default)]
pub struct Clock;

impl Clock {
    pub fn new() -> Self {
        Self
    }
}

/// Delegates time operations to the Tokio runtime.
impl crate::Clock for Clock {
    type Instant = tokio::time::Instant;
    type SleepFuture = tokio::time::Sleep;

    fn now(&self) -> Self::Instant {
        tokio::time::Instant::now()
    }

    fn sleep(&self, duration: std::time::Duration) -> Self::SleepFuture {
        tokio::time::sleep(duration)
    }
}

/// Bridges [`tokio::time::Instant`] into the [`InstantExt`](crate::InstantExt) abstraction.
impl crate::InstantExt for tokio::time::Instant {
    fn saturating_duration_since(&self, earlier: Self) -> std::time::Duration {
        self.checked_duration_since(earlier)
            .unwrap_or(std::time::Duration::ZERO)
    }
}
