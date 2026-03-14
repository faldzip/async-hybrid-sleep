//! Custom `Clock` example — a **`ScaledClock`** that speeds up or slows down
//! virtual time by a configurable multiplier.
//!
//! * `multiplier = 1.0` → real-time
//! * `multiplier = 2.0` → time flows twice as fast  (a 100 ms sleep finishes in ~50 ms)
//! * `multiplier = 0.5` → time flows twice as slow  (a 100 ms sleep finishes in ~200 ms)
//!
//! Run with:
//! ```sh
//! cargo run --example scaled_clock
//! ```

use std::time::{Duration, Instant};

use async_hybrid_sleep::{Clock, InstantExt};

// ── ScaledInstant ────────────────────────────────────────────────────────────

/// A newtype over [`std::time::Instant`] that represents a point in *virtual*
/// time.
///
/// All arithmetic (`Add`, `Sub`, `Ord`, …) is delegated to the inner instant so
/// that duration calculations between two `ScaledInstant`s "just work" — the
/// scaling is applied when the instant is *produced* by [`ScaledClock::now`],
/// not when it is compared or subtracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ScaledInstant(std::time::Instant);

impl std::ops::Add<Duration> for ScaledInstant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

impl std::ops::Sub<Duration> for ScaledInstant {
    type Output = Self;
    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

impl From<std::time::Instant> for ScaledInstant {
    fn from(i: std::time::Instant) -> Self {
        Self(i)
    }
}

impl From<ScaledInstant> for std::time::Instant {
    fn from(i: ScaledInstant) -> Self {
        i.0
    }
}

impl InstantExt for ScaledInstant {
    fn now() -> Self {
        // Fallback: in practice the `Clock::now()` method is used instead.
        Self(std::time::Instant::now())
    }

    fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }
}

// ── ScaledClock ──────────────────────────────────────────────────────────────

/// A [`Clock`] whose time flows at `multiplier ×` real speed.
///
/// Internally it records the *real* instant at which it was created
/// (`real_origin`) and a matching *virtual* origin.  Every call to
/// [`now()`](Clock::now) computes:
///
/// ```text
/// virtual_now = virtual_origin + (real_elapsed × multiplier)
/// ```
///
/// When asked to [`sleep(duration)`](Clock::sleep) it divides the requested
/// virtual duration by the multiplier so that the *real* wall-clock wait is
/// shorter (or longer) accordingly.
#[derive(Debug, Clone)]
struct ScaledClock {
    multiplier: f32,
    real_origin: std::time::Instant,
    virtual_origin: ScaledInstant,
}

impl ScaledClock {
    fn new(multiplier: f32) -> Self {
        assert!(multiplier > 0.0, "multiplier must be positive");
        let now = std::time::Instant::now();
        Self {
            multiplier,
            real_origin: now,
            virtual_origin: ScaledInstant(now),
        }
    }
}

/// `Default` is required by the [`Clock`] trait.  The default clock runs at
/// real speed.
impl Default for ScaledClock {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl Clock for ScaledClock {
    type Instant = ScaledInstant;
    type SleepFuture = tokio::time::Sleep;

    fn now(&self) -> ScaledInstant {
        let real_elapsed = self.real_origin.elapsed();
        let virtual_elapsed = real_elapsed.mul_f32(self.multiplier);
        self.virtual_origin + virtual_elapsed
    }

    fn sleep(&self, virtual_duration: Duration) -> tokio::time::Sleep {
        // Convert virtual duration → real wall-clock duration.
        let real_duration = virtual_duration.div_f32(self.multiplier);
        tokio::time::sleep(real_duration)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Runs a single one-shot sleep demo and prints timing info.
async fn demo_sleep(label: &str, clock: &ScaledClock, virtual_ms: u64) {
    let target = Duration::from_millis(virtual_ms);
    let wall_start = Instant::now();
    async_hybrid_sleep::sleep_with_clock(clock.clone(), target).await;
    let wall = wall_start.elapsed();
    println!(
        "  {label:<18}  virtual: {virtual_ms:>5}ms  \
         wall: {:>8.1}ms  \
         expected wall: {:>8.1}ms",
        wall.as_secs_f64() * 1000.0,
        target.as_secs_f64() / clock.multiplier as f64 * 1000.0,
    );
}

/// Runs an interval demo and prints per-tick timings.
async fn demo_interval(label: &str, clock: &ScaledClock, period_ms: u64, ticks: usize) {
    let period = Duration::from_millis(period_ms);
    let mut interval = async_hybrid_sleep::interval_with_clock(clock.clone(), period);

    let wall_start = Instant::now();
    let mut prev = wall_start;

    println!("  {label} — {ticks} ticks at {period_ms}ms virtual period:");
    for i in 1..=ticks {
        interval.tick().await;
        let now = Instant::now();
        let wall_delta = now.duration_since(prev);
        let expected_wall = period.div_f32(clock.multiplier);
        let error_us = wall_delta.as_micros() as i64 - expected_wall.as_micros() as i64;
        println!(
            "    tick {:>2}: wall delta = {:>8.3}ms  (error {:>+6}µs)",
            i,
            wall_delta.as_secs_f64() * 1000.0,
            error_us,
        );
        prev = now;
    }

    let total_wall = wall_start.elapsed();
    let expected_total = period.mul_f32(ticks as f32).div_f32(clock.multiplier);
    println!(
        "    total wall: {:.1}ms  (expected {:.1}ms)\n",
        total_wall.as_secs_f64() * 1000.0,
        expected_total.as_secs_f64() * 1000.0,
    );
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // ─── 1. One-shot sleeps at different speeds ──────────────────────────

    println!("=== One-shot sleep (virtual 100ms) at various speeds ===\n");

    for &mult in &[0.5, 1.0, 2.0, 5.0, 10.0] {
        let clock = ScaledClock::new(mult);
        demo_sleep(&format!("×{mult}"), &clock, 100).await;
    }

    // ─── 2. Several durations at 3× speed ───────────────────────────────

    println!("\n=== Various durations at ×3 speed ===\n");

    let fast = ScaledClock::new(3.0);
    for &ms in &[10, 50, 100, 500] {
        demo_sleep("×3", &fast, ms).await;
    }

    // ─── 3. Interval at 5× speed ────────────────────────────────────────

    println!("\n=== Interval at ×5 speed ===\n");

    let faster = ScaledClock::new(5.0);
    demo_interval("×5", &faster, 50, 8).await;

    // ─── 4. Interval at 0.5× (slow-motion) ──────────────────────────────

    println!("=== Interval at ×0.5 (slow-motion) ===\n");

    let slow = ScaledClock::new(0.5);
    demo_interval("×0.5", &slow, 20, 5).await;

    // ─── 5. Side-by-side comparison ──────────────────────────────────────

    println!("=== Side-by-side: 200ms virtual sleep ===\n");
    println!(
        "  {:>12}  {:>12}  {:>12}",
        "multiplier", "wall time", "expected"
    );
    println!(
        "  {:>12}  {:>12}  {:>12}",
        "----------", "---------", "--------"
    );

    for &mult in &[0.25, 0.5, 1.0, 2.0, 4.0, 10.0] {
        let clock = ScaledClock::new(mult);
        let virtual_dur = Duration::from_millis(200);
        let expected_wall = virtual_dur.div_f32(mult);
        let wall_start = Instant::now();
        async_hybrid_sleep::sleep_with_clock(clock, virtual_dur).await;
        let wall = wall_start.elapsed();
        println!(
            "  {:>12}  {:>10.1}ms  {:>10.1}ms",
            format!("×{mult}"),
            wall.as_secs_f64() * 1000.0,
            expected_wall.as_secs_f64() * 1000.0,
        );
    }

    println!();
}
