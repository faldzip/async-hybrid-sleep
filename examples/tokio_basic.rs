//! Basic usage examples with the Tokio runtime.
//!
//! Run with:
//! ```sh
//! cargo run --example tokio_basic
//! ```

use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    one_shot_sleep().await;
    sleep_until_deadline().await;
    interval_ticking().await;
    custom_threshold().await;
}

/// Simple one-shot sleep with accuracy measurement.
async fn one_shot_sleep() {
    println!("=== One-shot sleep ===");

    for &ms in &[1, 5, 50] {
        let target = Duration::from_millis(ms);
        let start = Instant::now();
        async_hybrid_sleep::sleep(target).await;
        let elapsed = start.elapsed();
        let error_us = elapsed.as_micros() as i64 - target.as_micros() as i64;
        println!(
            "  requested: {:>4}ms | actual: {:>10.3}ms | error: {:>+6}µs",
            ms,
            elapsed.as_secs_f64() * 1000.0,
            error_us,
        );
    }

    println!();
}

/// Sleep until an absolute deadline.
async fn sleep_until_deadline() {
    println!("=== Sleep until deadline ===");

    let now = async_hybrid_sleep::Instant::now();
    let deadline = now + Duration::from_millis(25);
    let wall_start = Instant::now();

    async_hybrid_sleep::sleep_until(deadline.into()).await;

    let elapsed = wall_start.elapsed();
    println!(
        "  deadline in 25ms | woke after {:.3}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
    println!();
}

/// Periodic interval demonstrating tick-to-tick consistency.
async fn interval_ticking() {
    println!("=== Interval (20ms period, 10 ticks) ===");

    let period = Duration::from_millis(20);
    let mut interval = async_hybrid_sleep::interval(period);

    let loop_start = Instant::now();
    let mut prev = loop_start;

    for i in 1..=10 {
        interval.tick().await;
        let now = Instant::now();
        let delta = now.duration_since(prev);
        let error_us = delta.as_micros() as i64 - period.as_micros() as i64;
        println!(
            "  tick {:>2}: delta = {:>8.3}ms  (error {:>+5}µs)",
            i,
            delta.as_secs_f64() * 1000.0,
            error_us,
        );
        prev = now;
    }

    let total = loop_start.elapsed();
    let expected = period * 10;
    println!(
        "  total: {:.3}ms (expected {:.0}ms, drift {:>+.3}ms)",
        total.as_secs_f64() * 1000.0,
        expected.as_secs_f64() * 1000.0,
        (total.as_secs_f64() - expected.as_secs_f64()) * 1000.0,
    );
    println!();
}

/// Using `SleepBuilder` with a custom spin-sleep threshold.
async fn custom_threshold() {
    println!("=== SleepBuilder with custom threshold ===");

    let target = Duration::from_millis(50);

    // Default threshold (10ms spin phase)
    let start = Instant::now();
    async_hybrid_sleep::sleep(target).await;
    let default_elapsed = start.elapsed();

    // Smaller threshold (2ms spin phase) — less CPU, slightly less precise
    let deadline = std::time::Instant::now() + target;
    let start = Instant::now();
    async_hybrid_sleep::sleep_builder(deadline)
        .threshold(Duration::from_millis(2))
        .build()
        .await;
    let small_elapsed = start.elapsed();

    // Larger threshold (25ms spin phase) — more CPU, more precise for short sleeps
    let deadline = std::time::Instant::now() + target;
    let start = Instant::now();
    async_hybrid_sleep::sleep_builder(deadline)
        .threshold(Duration::from_millis(25))
        .build()
        .await;
    let large_elapsed = start.elapsed();

    println!(
        "  threshold 10ms (default): {:.3}ms (error {:>+}µs)",
        default_elapsed.as_secs_f64() * 1000.0,
        default_elapsed.as_micros() as i64 - target.as_micros() as i64,
    );
    println!(
        "  threshold  2ms (smaller): {:.3}ms (error {:>+}µs)",
        small_elapsed.as_secs_f64() * 1000.0,
        small_elapsed.as_micros() as i64 - target.as_micros() as i64,
    );
    println!(
        "  threshold 25ms (larger):  {:.3}ms (error {:>+}µs)",
        large_elapsed.as_secs_f64() * 1000.0,
        large_elapsed.as_micros() as i64 - target.as_micros() as i64,
    );
    println!();
}
