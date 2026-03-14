//! Benchmarks comparing sleep accuracy between `async_hybrid_sleep` and native
//! `tokio::time::sleep`.
//!
//! Each benchmark sleeps for a target duration, then records the *actual* wall-clock
//! elapsed time.  Criterion will report the mean, standard deviation, etc. so you
//! can compare how close each implementation gets to the requested delay.
//!
//! Run with:
//! ```sh
//! cargo bench --bench sleep_accuracy
//! ```

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const DURATIONS: &[(u64, &str)] = &[(1, "1ms"), (5, "5ms"), (50, "50ms"), (1_000, "1s")];

fn interval_accuracy(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("failed to build tokio runtime");

    let period = Duration::from_millis(20);
    let tick_count = 100;

    let mut group = c.benchmark_group("interval_accuracy");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    // --- hybrid interval ---
    group.bench_function("hybrid_20ms", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut interval = async_hybrid_sleep::interval(period);
                    let start = Instant::now();
                    for _ in 0..tick_count {
                        interval.tick().await;
                    }
                    total += start.elapsed();
                }
                total
            })
        });
    });

    // --- tokio interval ---
    group.bench_function("tokio_20ms", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut interval = tokio::time::interval(period);
                    interval.tick().await; // tokio's first tick is instant
                    let start = Instant::now();
                    for _ in 0..tick_count {
                        interval.tick().await;
                    }
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.finish();

    // --- Detailed per-tick statistics (printed to stdout) ---
    eprintln!("\n===== Interval tick accuracy (target: 20ms, {tick_count} ticks) =====\n");

    for (label, run) in [("hybrid", true), ("tokio", false)] {
        let deviations: Vec<f64> = rt.block_on(async {
            let mut devs = Vec::with_capacity(tick_count);
            let target_us = period.as_micros() as f64;

            if run {
                // hybrid
                let mut interval = async_hybrid_sleep::interval(period);
                for _ in 0..tick_count {
                    let before = Instant::now();
                    interval.tick().await;
                    let elapsed_us = before.elapsed().as_micros() as f64;
                    devs.push(elapsed_us - target_us);
                }
            } else {
                // tokio
                let mut interval = tokio::time::interval(period);
                interval.tick().await; // first tick is instant
                for _ in 0..tick_count {
                    let before = Instant::now();
                    interval.tick().await;
                    let elapsed_us = before.elapsed().as_micros() as f64;
                    devs.push(elapsed_us - target_us);
                }
            }
            devs
        });

        let min = deviations.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = deviations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = deviations.iter().sum::<f64>() / deviations.len() as f64;
        let variance =
            deviations.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / deviations.len() as f64;
        let std_dev = variance.sqrt();
        let mut sorted = deviations.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = if sorted.len() % 2 == 0 {
            (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };

        eprintln!("  [{label}]");
        eprintln!("    min:     {min:>+10.1} µs");
        eprintln!("    max:     {max:>+10.1} µs");
        eprintln!("    mean:    {mean:>+10.1} µs");
        eprintln!("    median:  {median:>+10.1} µs");
        eprintln!("    std_dev: {std_dev:>10.1} µs");
        eprintln!();
    }
}

fn sleep_accuracy(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("failed to build tokio runtime");

    let mut group = c.benchmark_group("sleep_accuracy");

    // Lower sample sizes for longer sleeps to keep total bench time reasonable.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));

    for &(ms, label) in DURATIONS {
        let target = Duration::from_millis(ms);

        // --- hybrid sleep ---------------------------------------------------
        group.bench_with_input(BenchmarkId::new("hybrid", label), &target, |b, &target| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        async_hybrid_sleep::sleep(target).await;
                        total += start.elapsed();
                    }
                    total
                })
            });
        });

        // --- native tokio sleep ----------------------------------------------
        group.bench_with_input(BenchmarkId::new("tokio", label), &target, |b, &target| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        tokio::time::sleep(target).await;
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    group.finish();
}

fn reset_immediately_bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("failed to build tokio runtime");

    let period = Duration::from_millis(20);
    let reset_count: usize = 100;

    let mut group = c.benchmark_group("reset_immediately");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    // --- hybrid: total time for 100 reset_immediately + tick cycles ---
    group.bench_function("hybrid_total", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut interval = async_hybrid_sleep::interval(period);
                    interval.tick().await; // initial tick
                    let start = Instant::now();
                    for _ in 0..reset_count {
                        interval.reset_immediately();
                        interval.tick().await;
                    }
                    total += start.elapsed();
                }
                total
            })
        });
    });

    // --- tokio: total time for 100 reset_immediately + tick cycles ---
    group.bench_function("tokio_total", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut interval = tokio::time::interval(period);
                    interval.tick().await; // first tick is instant
                    interval.tick().await; // wait for first real tick
                    let start = Instant::now();
                    for _ in 0..reset_count {
                        interval.reset_immediately();
                        interval.tick().await;
                    }
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.finish();

    // --- Detailed per-reset statistics ---
    eprintln!("\n===== reset_immediately() latency ({reset_count} cycles) =====\n");

    for (label, use_hybrid) in [("hybrid", true), ("tokio", false)] {
        let durations_us: Vec<f64> = rt.block_on(async {
            let mut times = Vec::with_capacity(reset_count);

            if use_hybrid {
                let mut interval = async_hybrid_sleep::interval(period);
                interval.tick().await;
                for _ in 0..reset_count {
                    interval.reset_immediately();
                    let before = Instant::now();
                    interval.tick().await;
                    times.push(before.elapsed().as_nanos() as f64 / 1000.0);
                }
            } else {
                let mut interval = tokio::time::interval(period);
                interval.tick().await; // instant first tick
                interval.tick().await; // real tick
                for _ in 0..reset_count {
                    interval.reset_immediately();
                    let before = Instant::now();
                    interval.tick().await;
                    times.push(before.elapsed().as_nanos() as f64 / 1000.0);
                }
            }
            times
        });

        let total_us: f64 = durations_us.iter().sum();
        let min = durations_us.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = durations_us
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let mean = total_us / durations_us.len() as f64;
        let variance = durations_us.iter().map(|d| (d - mean).powi(2)).sum::<f64>()
            / durations_us.len() as f64;
        let std_dev = variance.sqrt();
        let mut sorted = durations_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = if sorted.len() % 2 == 0 {
            (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };

        eprintln!("  [{label}]");
        eprintln!("    total:   {total_us:>10.1} µs");
        eprintln!("    min:     {min:>10.3} µs");
        eprintln!("    max:     {max:>10.3} µs");
        eprintln!("    mean:    {mean:>10.3} µs");
        eprintln!("    median:  {median:>10.3} µs");
        eprintln!("    std_dev: {std_dev:>10.3} µs");
        eprintln!();
    }
}

criterion_group!(
    benches,
    sleep_accuracy,
    interval_accuracy,
    reset_immediately_bench
);
criterion_main!(benches);
