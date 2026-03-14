# async-hybrid-sleep

[![Crates.io](https://img.shields.io/crates/v/async-hybrid-sleep.svg)](https://crates.io/crates/async-hybrid-sleep)
[![Docs.rs](https://docs.rs/async-hybrid-sleep/badge.svg)](https://docs.rs/async-hybrid-sleep)

A hybrid async sleep implementation that combines runtime-native timers with spin-sleeping for precise, low-latency delays.

## The Problem

Async runtimes like Tokio and async-std rely on OS-level timers for their sleep implementations. These timers are efficient (zero CPU usage while waiting), but their resolution is limited by the OS scheduler — typically **1–2 ms** on Linux and up to **15 ms** on Windows. When you ask for a 1 ms sleep, you often get 2 ms or more.

For many applications this is fine, but latency-sensitive workloads — game loops, audio processing, real-time simulations, high-frequency trading, precise animation frames — need **sub-millisecond accuracy**.

## The Solution

`async-hybrid-sleep` uses a **two-phase strategy**:

1. **Phase 1 — Runtime timer:** For the bulk of the wait, the future delegates to the async runtime's native timer (`tokio::time::sleep` or `async_io::Timer`). This keeps CPU usage at zero for the long portion.
2. **Phase 2 — Spin-sleep:** When the remaining time drops below a configurable threshold (default: **10 ms**), the future switches to a spin-loop, repeatedly waking the task and checking the deadline until it's reached.

This gives you the best of both worlds: **efficient waiting** for most of the duration, and **precise completion** at the deadline.

```text
                  ┌─────── Runtime timer (idle CPU) ───────┐┌─ Spin (busy) ─┐
  sleep(100ms) :  ████████████████████████████████████████████░░░░░░░░░░░░░░░ → wakes at ≈100.000ms
                  0ms                                      90ms            100ms
```

## The `Clock` Trait

At the core of this crate is the [`Clock`] trait — a pluggable abstraction over how time is read and how timer futures are created:

```rust
pub trait Clock: Clone + Unpin + Sized + Default {
    type Instant: InstantExt;
    type SleepFuture: Future<Output = ()> + Send + 'static;

    fn now(&self) -> Self::Instant;
    fn sleep(&self, duration: Duration) -> Self::SleepFuture;
}
```

Every type in this crate — `Sleep<C>`, `Interval<C>`, `SleepBuilder<C>` — is generic over `C: Clock`. This design decouples the hybrid sleep logic from any specific async runtime and opens the door to powerful custom implementations:

- **Scaled clock** — a clock whose `now()` advances faster or slower than real time, letting you speed up simulations or create slow-motion replays without changing any sleep durations in your application code.
- **Mock / deterministic clock** — a clock that only advances when you explicitly tell it to, perfect for unit-testing time-dependent logic without actual waiting.
- **Replay clock** — a clock that follows a pre-recorded timeline, useful for replaying captured event streams at their original pace.
- **Offset clock** — a clock that adds a fixed offset to wall-clock time, handy for testing behavior around time boundaries (midnight, DST transitions, etc.).

The crate ships two built-in implementations behind feature flags:

| Type           | Feature      | Backend               |
| -------------- | ------------ | --------------------- |
| `TokioClock`   | `tokio`      | `tokio::time`         |
| `AsyncIoClock` | `async-io`   | `async_io::Timer`     |

### Using a custom clock

All `_with_clock` functions accept any `C: Clock`:

```rust
// With a custom clock (e.g. a ScaledClock running at 5× speed):
let clock = ScaledClock::new(5.0);
async_hybrid_sleep::sleep_with_clock(clock.clone(), Duration::from_millis(100)).await;

let mut interval = async_hybrid_sleep::interval_with_clock(clock, Duration::from_millis(20));
interval.tick().await;
```

`SleepBuilder` works the same way — just supply the clock type:

```rust
let deadline = std::time::Instant::now() + Duration::from_millis(200);
SleepBuilder::<ScaledClock>::new(deadline)
    .clock(ScaledClock::new(2.0))
    .threshold(Duration::from_millis(5))
    .build()
    .await;
```

See the [`examples/scaled_clock.rs`](examples/scaled_clock.rs) example for a complete, runnable custom `Clock` implementation.

## Benchmark Results

All benchmarks were run on Linux with a Tokio current-thread runtime using [Criterion](https://github.com/bheisler/criterion.rs). Measured values represent actual wall-clock time elapsed for the requested sleep duration.

### Sleep Accuracy

| Requested | `async-hybrid-sleep` | `tokio::time::sleep` | Overshoot (hybrid) | Overshoot (tokio) |
| --------- | -------------------: | -------------------: | -----------------: | ----------------: |
| **1 ms**  |            1.0001 ms |            2.0558 ms |        **~0.1 µs** |         ~1,056 µs |
| **5 ms**  |            5.0001 ms |            6.0815 ms |        **~0.1 µs** |         ~1,082 µs |
| **50 ms** |            50.001 ms |            51.060 ms |          **~1 µs** |         ~1,060 µs |
| **1 s**   |             1.0000 s |             1.0010 s |          **~0 µs** |         ~1,000 µs |

### Interval Tick Accuracy (20 ms period, 100 ticks)

| Metric     | `async-hybrid-sleep` | `tokio::time::interval` |
| ---------- | -------------------: | ----------------------: |
| Mean error |          **−0.3 µs** |                 −4.7 µs |
| Median     |           **0.0 µs** |                +69.5 µs |
| Std dev    |           **0.5 µs** |                298.8 µs |
| Min        |              −2.0 µs |               −943.0 µs |
| Max        |              +1.0 µs |               +666.0 µs |

### `reset_immediately()` Latency (100 cycles)

| Metric | `async-hybrid-sleep` | `tokio::time::interval` |
| ------ | -------------------: | ----------------------: |
| Mean   |         **0.064 µs** |              1,055.8 µs |
| Median |         **0.060 µs** |              1,055.8 µs |
| Total  |           **6.4 µs** |              105,583 µs |

## Trade-offs

|                                    | Hybrid sleep                       | Native runtime sleep                    |
| ---------------------------------- | ---------------------------------- | --------------------------------------- |
| ✅ Sub-millisecond accuracy        | Yes                                | No                                      |
| ✅ Low CPU usage during long waits | Yes                                | Yes                                     |
| ✅ Zero CPU during entire wait     | No                                 | Yes                                     |
| ⚠️ CPU usage during spin phase     | ~10 ms of busy-wait per sleep      | None                                    |
| ⚠️ Best suited for                 | Latency-sensitive, real-time tasks | General-purpose, battery-friendly tasks |

### When to use this crate

- **Game loops** that need a consistent 60/120/144 Hz tick rate
- **Audio/video processing** with strict frame timing
- **Real-time simulations** where drift accumulates over time
- **High-frequency polling** or coordination tasks
- Any workload where **±1 ms jitter is unacceptable**

### When NOT to use this crate

- General-purpose application timers where millisecond precision doesn't matter
- Battery-powered / embedded devices where CPU wake-ups are costly
- Scenarios with thousands of concurrent sleeps (each spin phase occupies a core)

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
async-hybrid-sleep = "0.1"
```

### One-shot sleep

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Sleeps for exactly 16.667ms (60 Hz frame time)
    async_hybrid_sleep::sleep(Duration::from_micros(16_667)).await;
}
```

### Interval (periodic tick)

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut interval = async_hybrid_sleep::interval(Duration::from_millis(16));
    loop {
        interval.tick().await;
        // runs at ~62.5 Hz with sub-microsecond jitter
    }
}
```

### Sleep until a deadline

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    let deadline = async_hybrid_sleep::Instant::now() + Duration::from_millis(100);
    async_hybrid_sleep::sleep_until(deadline).await;
}
```

### Custom spin threshold

Use `SleepBuilder` to tune how early the spin phase kicks in. A smaller threshold means less CPU usage but slightly lower accuracy:

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    let deadline = async_hybrid_sleep::Instant::now() + Duration::from_millis(200);
    let sleep = async_hybrid_sleep::SleepBuilder::new(deadline)
        .threshold(Duration::from_millis(5)) // spin only the last 5ms
        .build();
    sleep.await;
}
```

## Runtime Support & Feature Flags

The crate provides four feature flags related to runtime selection:

| Feature    | Runtime         | Default |
| ---------- | --------------- | ------- |
| `tokio`    | Tokio           | ✅      |
| `async-io` | async-io        |         |
| `smol`     | smol (async-io) |         |

### `DefaultClock` and convenience functions

The availability of `DefaultClock` (and the convenience functions `sleep`, `sleep_until`, `interval`, and `sleep_builder` that rely on it) depends on which features are enabled:

| Features enabled           | `DefaultClock` | Convenience functions | Notes                                           |
| -------------------------- | :------------: | :-------------------: | ----------------------------------------------- |
| `tokio` only *(default)*   |       ✅       |          ✅           | `DefaultClock` wraps `TokioClock`               |
| `async-io` only            |       ✅       |          ✅           | `DefaultClock` wraps `AsyncIoClock`              |
| Both `tokio` + `async-io`  |       ❌       |          ❌           | Ambiguous — use explicit clock types instead    |
| Neither                    |       ❌       |          ❌           | No runtime — use a custom `Clock` implementation |

When `DefaultClock` is **not** available, use the explicit `_with_clock` variants and supply a clock type directly:

- `sleep_with_clock(clock, duration)`
- `sleep_until_with_clock(clock, deadline)`
- `interval_with_clock(clock, period)`
- `SleepBuilder::<C>::new(deadline)`

### Single runtime (default)

```toml
# Tokio (default — nothing extra needed)
[dependencies]
async-hybrid-sleep = "0.1"
```

```toml
# async-io instead of Tokio
[dependencies]
async-hybrid-sleep = { version = "0.1", default-features = false, features = ["async-io"] }
```

With a single runtime, all convenience functions work out of the box:

```rust
async_hybrid_sleep::sleep(Duration::from_millis(100)).await;
```

### Both runtimes enabled

When you enable **both** `tokio` and `async-io`, the crate cannot pick a default, so `DefaultClock` is not defined. You must use explicit clock types — `TokioClock` or `AsyncIoClock`:

```toml
[dependencies]
async-hybrid-sleep = { version = "0.1", features = ["tokio", "async-io"] }
```

```rust
use async_hybrid_sleep::{TokioClock, AsyncIoClock};

// Explicitly choose which runtime backs the sleep:
async_hybrid_sleep::sleep_with_clock(TokioClock::default(), Duration::from_millis(50)).await;
async_hybrid_sleep::sleep_with_clock(AsyncIoClock::default(), Duration::from_millis(50)).await;

// SleepBuilder requires a type parameter:
let deadline = std::time::Instant::now() + Duration::from_millis(200);
async_hybrid_sleep::SleepBuilder::<TokioClock>::new(deadline)
    .threshold(Duration::from_millis(5))
    .build()
    .await;
```

See [`examples/dual_runtime.rs`](examples/dual_runtime.rs) for a complete example.

### No runtime features (custom clock only)

If you disable all default features and don't enable any runtime, you can still use the crate with your own `Clock` implementation:

```toml
[dependencies]
async-hybrid-sleep = { version = "0.1", default-features = false }
```

```rust
use async_hybrid_sleep::{Clock, InstantExt};

#[derive(Clone, Default)]
struct MyClock { /* ... */ }

impl Clock for MyClock {
    type Instant = std::time::Instant;
    type SleepFuture = /* ... */;

    fn now(&self) -> Self::Instant { /* ... */ }
    fn sleep(&self, duration: Duration) -> Self::SleepFuture { /* ... */ }
}

// Then use the _with_clock APIs:
let clock = MyClock::default();
async_hybrid_sleep::sleep_with_clock(clock, Duration::from_millis(100)).await;
```

## Running Benchmarks

```sh
cargo bench --bench sleep_accuracy
```

This runs Criterion benchmarks comparing hybrid vs. native sleep accuracy for 1 ms, 5 ms, 50 ms, and 1 s durations, plus interval tick accuracy and `reset_immediately()` latency.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option.