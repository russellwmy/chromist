//! Sustained-load benchmark for the command-dispatch hot path.
//!
//! Measures `CommandDispatcher::register`, `resolve`, and `sweep_timeouts`
//! throughput at realistic in-flight concurrency levels. Run with:
//!
//! ```sh
//! cargo bench -p chromist --features _bench --bench dispatch
//! ```
//!
//! Every outbound CDP call walks `register`; every inbound response walks
//! `resolve`; the 500 ms sweep tick walks `sweep_timeouts` linearly over all
//! active entries. These benches pin the cost at different pending-call counts
//! so regressions surface as wall-clock, not a gut feel.

use std::time::Duration;

use chromist::__bench::CommandDispatcher;
use chromist_types::CallId;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::channel::oneshot;
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Cost of a single `register` on top of N already-pending entries.
///
/// `b.iter_batched` rebuilds the populated dispatcher each iteration so the
/// pending-set size stays fixed at N when we measure the insert.
fn bench_register(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch/register");
    group.throughput(Throughput::Elements(1));

    for &n in &[0usize, 16, 256, 4096] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut disp = CommandDispatcher::new(TIMEOUT);
                    let mut keepalive = Vec::with_capacity(n);
                    for i in 0..n {
                        let (tx, rx) = oneshot::channel();
                        disp.register(CallId::new(i), tx);
                        keepalive.push(rx);
                    }
                    (disp, keepalive, n)
                },
                |(mut disp, keepalive, n)| {
                    let (tx, _rx) = oneshot::channel();
                    disp.register(CallId::new(n), tx);
                    // Keep receivers alive so senders don't see Disconnected mid-bench.
                    std::hint::black_box(keepalive);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Cost of resolving one call by id with N pending entries in the map.
///
/// `resolve` must do one `FnvHashMap::remove` and one `oneshot::Sender::send`;
/// both are O(1) but degrade with table size due to cache effects.
fn bench_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch/resolve");
    group.throughput(Throughput::Elements(1));

    for &n in &[1usize, 16, 256, 4096] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut disp = CommandDispatcher::new(TIMEOUT);
                    let mut keepalive = Vec::with_capacity(n);
                    for i in 0..n {
                        let (tx, rx) = oneshot::channel();
                        disp.register(CallId::new(i), tx);
                        keepalive.push(rx);
                    }
                    // Target id lives in the middle of the set to avoid landing
                    // on the most-recently-inserted slot every iteration.
                    let target = CallId::new(n / 2);
                    (disp, keepalive, target)
                },
                |(mut disp, keepalive, target)| {
                    disp.resolve(target, Ok(Value::Null));
                    std::hint::black_box(keepalive);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Cost of one sweep over N pending entries where **none** have expired — the
/// steady-state 500 ms tick in a healthy session. Exercises the O(n) scan
/// without the removal side of the loop.
fn bench_sweep_no_expired(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch/sweep_no_expired");
    group.throughput(Throughput::Elements(1));

    for &n in &[0usize, 16, 256, 4096] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut disp = CommandDispatcher::new(TIMEOUT);
            let mut keepalive = Vec::with_capacity(n);
            for i in 0..n {
                let (tx, rx) = oneshot::channel();
                disp.register(CallId::new(i), tx);
                keepalive.push(rx);
            }

            b.iter(|| {
                disp.sweep_timeouts();
            });

            std::hint::black_box(keepalive);
        });
    }
    group.finish();
}

/// Cost of one sweep where **all** N entries have already passed their
/// deadline — the worst case for teardown when the connection is stalled.
/// Exercises both the scan and the removal loop.
fn bench_sweep_all_expired(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch/sweep_all_expired");
    group.throughput(Throughput::Elements(1));

    for &n in &[1usize, 16, 256, 4096] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    // `Duration::ZERO` makes every deadline already-past on registration.
                    let mut disp = CommandDispatcher::new(Duration::ZERO);
                    let mut keepalive = Vec::with_capacity(n);
                    for i in 0..n {
                        let (tx, rx) = oneshot::channel();
                        disp.register(CallId::new(i), tx);
                        keepalive.push(rx);
                    }
                    (disp, keepalive)
                },
                |(mut disp, keepalive)| {
                    disp.sweep_timeouts();
                    std::hint::black_box(keepalive);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_register,
    bench_resolve,
    bench_sweep_no_expired,
    bench_sweep_all_expired
);
criterion_main!(benches);
