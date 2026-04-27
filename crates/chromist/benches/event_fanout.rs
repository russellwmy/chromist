//! Sustained-load benchmark for the event fan-out path.
//!
//! Measures `EventBus::publish` throughput across realistic subscriber
//! topologies. Run with:
//!
//! ```sh
//! cargo bench -p chromist --features _bench --bench event_fanout
//! ```
//!
//! The fan-out path is the hottest one in chromist under load: every CDP
//! event the browser emits walks every global subscriber and every matching
//! per-session subscriber, trying a bounded `mpsc::Sender::try_send` against
//! each. These benches pin the cost at different subscriber counts so
//! regressions surface as wall-clock, not a gut feel.

use std::sync::Arc;

use chromist::__bench::{EventBus, EventFrame, EVENT_CHANNEL_CAP};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::channel::mpsc;

fn make_frame(method: &'static str, session_id: Option<Arc<str>>) -> Arc<EventFrame> {
    Arc::new(EventFrame {
        method: method.to_string(),
        params: serde_json::json!({
            "requestId": "abc-123",
            "loaderId": "loader-1",
            "timestamp": 1234.567,
            "type": "Document",
            "frameId": "frame-0",
            "hasExtraInfo": false,
        }),
        session_id,
    })
}

/// Register `n` global subscribers with their receivers held so senders stay
/// alive for the duration of the bench.
fn register_global(bus: &mut EventBus, n: usize) -> Vec<mpsc::Receiver<Arc<EventFrame>>> {
    (0..n)
        .map(|_| {
            let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
            bus.subscribe_with_sender(None, tx);
            rx
        })
        .collect()
}

fn register_sessions(
    bus: &mut EventBus,
    sessions: &[Arc<str>],
    subs_per_session: usize,
) -> Vec<mpsc::Receiver<Arc<EventFrame>>> {
    let mut rxs = Vec::with_capacity(sessions.len() * subs_per_session);
    for sid in sessions {
        for _ in 0..subs_per_session {
            let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
            bus.subscribe_with_sender(Some(Arc::clone(sid)), tx);
            rxs.push(rx);
        }
    }
    rxs
}

/// Drain every receiver so its channel never fills — we're measuring
/// steady-state fan-out, not backpressure-drop.
fn drain(rxs: &mut [mpsc::Receiver<Arc<EventFrame>>]) {
    for rx in rxs.iter_mut() {
        while rx.try_recv().is_ok() {}
    }
}

/// Publish throughput at N global subscribers (the fan-out width that matters
/// for broadcast events like `Page.*` and `Target.*`).
fn bench_global_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout/global");
    group.throughput(Throughput::Elements(1));

    for &n in &[0usize, 1, 4, 16, 64, 256] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut bus = EventBus::new();
            let mut rxs = register_global(&mut bus, n);
            let frame = make_frame("Page.loadEventFired", None);

            b.iter(|| {
                bus.publish(Arc::clone(&frame));
                drain(&mut rxs);
            });
        });
    }
    group.finish();
}

/// Per-session routing cost. Events are tagged with one of S session IDs and
/// each session has a subscriber; measures how a session lookup scales.
fn bench_session_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout/session");
    group.throughput(Throughput::Elements(1));

    for &s in &[1usize, 8, 64, 256] {
        let sessions: Vec<Arc<str>> =
            (0..s).map(|i| Arc::from(format!("session-{i}").as_str())).collect();

        group.bench_with_input(BenchmarkId::from_parameter(s), &s, |b, _| {
            let mut bus = EventBus::new();
            let mut rxs = register_sessions(&mut bus, &sessions, 1);
            let target = Arc::clone(&sessions[s / 2]);
            let frame = make_frame("Network.requestWillBeSent", Some(target));

            b.iter(|| {
                bus.publish(Arc::clone(&frame));
                drain(&mut rxs);
            });
        });
    }
    group.finish();
}

/// Mixed workload: a handful of global subscribers (e.g. a metrics sink) plus
/// N session subscribers. Approximates a real chromist client running a
/// network manager alongside per-page typed streams.
fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout/mixed");
    group.throughput(Throughput::Elements(1));

    let globals = 2usize;
    for &pages in &[1usize, 8, 32, 128] {
        let sessions: Vec<Arc<str>> =
            (0..pages).map(|i| Arc::from(format!("page-{i}").as_str())).collect();

        group.bench_with_input(BenchmarkId::from_parameter(pages), &pages, |b, _| {
            let mut bus = EventBus::new();
            let mut rxs = register_global(&mut bus, globals);
            rxs.extend(register_sessions(&mut bus, &sessions, 3));
            let target = Arc::clone(&sessions[pages / 2]);
            let frame = make_frame("Network.responseReceived", Some(target));

            b.iter(|| {
                bus.publish(Arc::clone(&frame));
                drain(&mut rxs);
            });
        });
    }
    group.finish();
}

/// Disconnected-subscriber pruning cost. Fills the global pool with senders
/// whose receivers have been dropped; every publish call must prune them.
/// Exercises the `Err(_) => false` branch in `fan_out`'s `retain_mut`.
fn bench_prune_disconnected(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout/prune_dead");
    group.throughput(Throughput::Elements(1));

    for &dead in &[1usize, 16, 128] {
        group.bench_with_input(BenchmarkId::from_parameter(dead), &dead, |b, &dead| {
            b.iter_batched(
                || {
                    let mut bus = EventBus::new();
                    for _ in 0..dead {
                        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
                        bus.subscribe_with_sender(None, tx);
                        drop(rx); // make the sender's try_send fail with Disconnected
                    }
                    bus
                },
                |mut bus| {
                    bus.publish(make_frame("Page.loadEventFired", None));
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_global_fanout,
    bench_session_fanout,
    bench_mixed,
    bench_prune_disconnected
);
criterion_main!(benches);
