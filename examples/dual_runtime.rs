//! Example using **both** `tokio` and `async-io` clocks side-by-side.
//!
//! When both runtime features are enabled simultaneously there is no
//! `DefaultClock`, so all APIs require an explicit clock type.  This
//! example demonstrates:
//!
//! * `sleep_with_clock` / `interval_with_clock` with each backend
//! * `SleepBuilder::<TokioClock>` and `SleepBuilder::<AsyncIoClock>`
//!
//! Run with:
//! ```sh
//! cargo run --example dual_runtime --features "tokio,async-io"
//! ```

#[cfg(not(all(feature = "tokio", feature = "async-io")))]
fn main() {
    eprintln!(
        "This example requires both runtime features.\n\
         Re-run with: cargo run --example dual_runtime --features \"tokio,async-io\""
    );
}

#[cfg(all(feature = "tokio", feature = "async-io"))]
use std::time::{Duration, Instant};

#[cfg(all(feature = "tokio", feature = "async-io"))]
use async_hybrid_sleep::{AsyncIoClock, Clock, SleepBuilder, TokioClock};

// ── Helpers ──────────────────────────────────────────────────────────────────

#[cfg(all(feature = "tokio", feature = "async-io"))]
async fn demo_sleep<C: Clock>(label: &str, clock: C, durations: &[u64]) {
    println!("  {label}:");
    for &ms in durations {
        let target = Duration::from_millis(ms);
        let wall_start = Instant::now();
        async_hybrid_sleep::sleep_with_clock(clock.clone(), target).await;
        let wall = wall_start.elapsed();
        let error_us = wall.as_micros() as i64 - target.as_micros() as i64;
        println!(
            "    {:>5}ms → {:>10.3}ms  (error {:>+6}µs)",
            ms,
            wall.as_secs_f64() * 1000.0,
            error_us,
        );
    }
    println!();
}

#[cfg(all(feature = "tokio", feature = "async-io"))]
async fn demo_interval<C: Clock>(label: &str, clock: C, period_ms: u64, ticks: usize) {
    let period = Duration::from_millis(period_ms);
    let mut interval = async_hybrid_sleep::interval_with_clock(clock, period);

    println!("  {label} — {ticks} ticks at {period_ms}ms period:");
    let loop_start = Instant::now();
    let mut prev = loop_start;

    for i in 1..=ticks {
        interval.tick().await;
        let now = Instant::now();
        let delta = now.duration_since(prev);
        let error_us = delta.as_micros() as i64 - period.as_micros() as i64;
        println!(
            "    tick {:>2}: delta = {:>8.3}ms  (error {:>+6}µs)",
            i,
            delta.as_secs_f64() * 1000.0,
            error_us,
        );
        prev = now;
    }

    let total = loop_start.elapsed();
    let expected = period * ticks as u32;
    println!(
        "    total: {:.3}ms  (expected {:.0}ms, drift {:>+.3}ms)\n",
        total.as_secs_f64() * 1000.0,
        expected.as_secs_f64() * 1000.0,
        (total.as_secs_f64() - expected.as_secs_f64()) * 1000.0,
    );
}

#[cfg(all(feature = "tokio", feature = "async-io"))]
async fn demo_builder<C: Clock>(label: &str, target_ms: u64, threshold_ms: u64) {
    let target = Duration::from_millis(target_ms);
    let deadline = std::time::Instant::now() + target;
    let wall_start = Instant::now();

    SleepBuilder::<C>::new(deadline)
        .threshold(Duration::from_millis(threshold_ms))
        .build()
        .await;

    let wall = wall_start.elapsed();
    let error_us = wall.as_micros() as i64 - target.as_micros() as i64;
    println!(
        "    {label:<12}  threshold: {:>3}ms  sleep: {:>5}ms → {:>10.3}ms  (error {:>+6}µs)",
        threshold_ms,
        target_ms,
        wall.as_secs_f64() * 1000.0,
        error_us,
    );
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[cfg(all(feature = "tokio", feature = "async-io"))]
#[tokio::main]
async fn main() {
    // ─── 1. One-shot sleeps ──────────────────────────────────────────────

    println!("=== One-shot sleep: Tokio vs async-io ===\n");

    let durations = &[1, 5, 50, 200];

    demo_sleep("TokioClock", TokioClock::default(), durations).await;
    demo_sleep("AsyncIoClock", AsyncIoClock::default(), durations).await;

    // ─── 2. Intervals ───────────────────────────────────────────────────

    println!("=== Interval: Tokio vs async-io ===\n");

    demo_interval("TokioClock", TokioClock::default(), 20, 8).await;
    demo_interval("AsyncIoClock", AsyncIoClock::default(), 20, 8).await;

    // ─── 3. SleepBuilder with explicit clock types ──────────────────────

    println!("=== SleepBuilder<TokioClock> vs SleepBuilder<AsyncIoClock> ===\n");

    for &threshold in &[2, 10, 25] {
        demo_builder::<TokioClock>("TokioClock", 50, threshold).await;
        demo_builder::<AsyncIoClock>("AsyncIoClock", 50, threshold).await;
        println!();
    }

    // ─── 4. Side-by-side comparison table ────────────────────────────────

    println!("=== Side-by-side: 100ms sleep ===\n");
    println!("  {:>14}  {:>12}  {:>12}", "clock", "wall time", "error");
    println!(
        "  {:>14}  {:>12}  {:>12}",
        "--------------", "----------", "----------"
    );

    let target = Duration::from_millis(100);

    for (label, is_tokio) in [("TokioClock", true), ("AsyncIoClock", false)] {
        let wall_start = Instant::now();
        if is_tokio {
            async_hybrid_sleep::sleep_with_clock(TokioClock::default(), target).await;
        } else {
            async_hybrid_sleep::sleep_with_clock(AsyncIoClock::default(), target).await;
        }
        let wall = wall_start.elapsed();
        let error_us = wall.as_micros() as i64 - target.as_micros() as i64;
        println!(
            "  {:>14}  {:>10.3}ms  {:>+10}µs",
            label,
            wall.as_secs_f64() * 1000.0,
            error_us,
        );
    }

    println!();
}
