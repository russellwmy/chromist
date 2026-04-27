use chromist_cdp::cdp::events::CdpEventMessage;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

// Representative CDP event payloads covering common hot paths.
static NAVIGATE: &str = r#"{"method":"Page.frameNavigated","sessionId":"s1","params":{"frame":{"id":"f1","loaderId":"l1","url":"https://example.com/","securityOrigin":"https://example.com","mimeType":"text/html"},"type":"Navigation"}}"#;

static REQUEST_WILL_BE_SENT: &str = r#"{"method":"Network.requestWillBeSent","sessionId":"s1","params":{"requestId":"1","loaderId":"l1","documentURL":"https://example.com/","request":{"url":"https://example.com/","method":"GET","headers":{},"initialPriority":"VeryHigh","referrerPolicy":"strict-origin-when-cross-origin"},"timestamp":1.0,"wallTime":1.0,"initiator":{"type":"other"},"redirectHasExtraInfo":false,"type":"Document","frameId":"f1","hasUserGesture":false}}"#;

static LIFECYCLE: &str = r#"{"method":"Page.lifecycleEvent","sessionId":"s1","params":{"frameId":"f1","loaderId":"l1","name":"load","timestamp":1.0}}"#;

static UNKNOWN_EVENT: &str =
    r#"{"method":"Future.newFeature","sessionId":"s1","params":{"value":42}}"#;

fn bench_event_deser(c: &mut Criterion) {
    let cases: &[(&str, &str)] = &[
        ("Page.frameNavigated", NAVIGATE),
        ("Network.requestWillBeSent", REQUEST_WILL_BE_SENT),
        ("Page.lifecycleEvent", LIFECYCLE),
        ("unknown_event", UNKNOWN_EVENT),
    ];

    let mut group = c.benchmark_group("event_deser");
    for (name, payload) in cases {
        let bytes = payload.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(BenchmarkId::new("deserialize", name), payload, |b, input| {
            b.iter(|| serde_json::from_str::<CdpEventMessage>(input).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_event_deser);
criterion_main!(benches);
