//! Benchmark for the `EventStream::poll_next` filter + deserialize hot path.
//!
//! Run with:
//!
//! ```sh
//! cargo bench -p chromist --bench event_stream
//! ```

use std::sync::Arc;

use chromist::{EventFrame, EventStream};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::StreamExt;

#[derive(serde::Deserialize, Debug)]
struct BenchEvent {
    #[allow(dead_code)]
    value: i32,
}

impl chromist_types::MethodType for BenchEvent {
    fn method_id() -> chromist_types::MethodId {
        std::borrow::Cow::Borrowed("Bench.event")
    }
}

impl chromist_types::Method for BenchEvent {
    fn identifier(&self) -> chromist_types::MethodId {
        std::borrow::Cow::Borrowed("Bench.event")
    }
}

impl chromist_types::EventMessage for BenchEvent {}

fn make_frame(method: &str, params: serde_json::Value) -> Arc<EventFrame> {
    Arc::new(EventFrame { method: method.to_string(), params, session_id: None })
}

/// Drain N matching frames from a pre-filled channel.
/// Measures combined filter + deserialize throughput.
fn bench_all_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_stream/all_match");

    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();

    for &n in &[1usize, 8, 64, 256] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let (mut tx, rx) = futures::channel::mpsc::channel(n + 1);
                    let mut stream: EventStream<BenchEvent> = EventStream::new(rx);
                    for _ in 0..n {
                        tx.try_send(make_frame("Bench.event", serde_json::json!({"value": 0})))
                            .unwrap();
                    }
                    drop(tx);
                    while stream.next().await.is_some() {}
                });
            });
        });
    }
    group.finish();
}

/// Drain N non-matching frames from a pre-filled channel (filter-only path).
/// Closes the channel after pre-filling so the stream returns None after skipping all.
fn bench_all_skip(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_stream/all_skip");

    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();

    for &n in &[1usize, 8, 64, 256] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let (mut tx, rx) = futures::channel::mpsc::channel(n + 1);
                    let mut stream: EventStream<BenchEvent> = EventStream::new(rx);
                    for _ in 0..n {
                        tx.try_send(make_frame("Other.event", serde_json::json!({"value": 0})))
                            .unwrap();
                    }
                    drop(tx);
                    // After all non-matching frames are skipped the channel is
                    // closed, so next() returns None immediately.
                    while stream.next().await.is_some() {}
                });
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_all_match, bench_all_skip);
criterion_main!(benches);
