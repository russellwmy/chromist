# Testing Guide — chromist

How to write and run tests in this workspace. Two tiers exist: synchronous unit tests (no runtime needed) and async integration tests (require a real Chromium binary).

---

## Test tiers

### Tier 1 — Synchronous unit tests (`#[test]`)

Use for all logic that does not require I/O: protocol math, channel semantics, error formatting, serde round-trips, PDL parsing, code generation. No `#[tokio::test]`, no `async fn`. These run in every `cargo test` invocation.

**Examples in the codebase:**
- `dispatch.rs` — `CommandDispatcher` register/resolve/sweep/fail_all (12 tests, no I/O)
- `event_bus.rs` — fan-out, session filtering, backpressure drop, pruning
- `error.rs` — `Display` formatting, `From` conversions
- `chromist-types/src/lib.rs` — wire type serde, `CallId` ordering, `Binary` round-trip
- `chromist-cdp/tests/serde_roundtrip.rs` — generated type round-trips
- `chromist-pdl/tests/parse_protocol.rs` — PDL parser on the vendored PDL files

### Tier 2 — Async integration tests (`#[tokio::test]`)

Use for end-to-end flows that require a real browser (navigate, screenshot, evaluate JS). These live in `crates/chromist/tests/integration.rs` and are **gated behind the `integration-tests` feature flag** so CI can skip them when no browser is available.

```sh
# Run integration tests (requires Chromium on PATH)
cargo test -p chromist --test integration --features integration-tests

# Run a single integration test
cargo test -p chromist --test integration --features integration-tests evaluate_expression
```

The feature flag is declared in `crates/chromist/Cargo.toml`:
```toml
[features]
integration-tests = []
```

The test file begins with `#![cfg(feature = "integration-tests")]` so it compiles away entirely without the flag.

---

## Running tests

```sh
# All tests except integration (default)
cargo test --workspace

# Single crate
cargo test -p chromist-types
cargo test -p chromist-pdl
cargo test -p chromist-cdp
cargo test -p chromist

# Single test by name (substring match)
cargo test -p chromist dispatch::tests::test_register_and_resolve_success

# Single test — show stdout even on pass
cargo test -p chromist test_fail_all -- --nocapture
```

---

## Testing channel-based code synchronously

`futures::channel::oneshot` and `mpsc` receivers have a `try_recv()` method that returns immediately without blocking. Use this to assert on channel state in synchronous tests — no runtime needed.

```rust
// Pattern from dispatch.rs
fn recv_now<T>(mut rx: oneshot::Receiver<T>) -> T {
    match rx.try_recv() {
        Ok(Some(v)) => v,
        other => panic!("expected Ok(Some(_)), got: {:?}", other.map(|o| o.map(|_| "<val>"))),
    }
}

#[test]
fn test_register_and_resolve_success() {
    let mut disp = CommandDispatcher::new(Duration::from_secs(30));
    let (tx, rx) = oneshot::channel();
    disp.register(CallId::new(1), tx);
    disp.resolve(CallId::new(1), Ok(Value::Null));
    assert!(recv_now(rx).is_ok());
}
```

The same `try_recv()` pattern works for `mpsc::Receiver` — see `event_bus.rs` tests.

---

## Testing the Handler without a real browser

`MockConnection` exists in `conn.rs` for this purpose. `MockConnection::pair()` returns `(MockConnection, MockHandle)`. Construct a `Handler` with the mock connection and drive the loop by sending synthetic inbound messages through `MockHandle`. This lets you test `handle_incoming`, target tree updates, and `fail_all` without any network I/O.

---

## Where tests live

| Location | What goes there |
|---|---|
| `src/<module>.rs` inline `#[cfg(test)]` | Tests for that module's internal logic; can access private items |
| `crates/<name>/tests/*.rs` | Black-box tests of the crate's public API |
| `crates/chromist/tests/integration.rs` | Real-browser end-to-end tests; gated by `integration-tests` feature |

**Rule:** if a test needs access to private fields or `pub(crate)` items, put it inline. If it tests only the public API, put it in `tests/`. Never put integration tests inline — they belong in `tests/integration.rs` with the feature gate.

---

## Serde round-trip tests for generated types

When adding new generated types or changing the generator, add a round-trip test in `crates/chromist-cdp/tests/serde_roundtrip.rs`:

```rust
#[test]
fn my_params_roundtrip() {
    let params = cdp::browser_protocol::my_domain::MyParams::new("value");
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["fieldName"], "value");
    let back: cdp::browser_protocol::my_domain::MyParams =
        serde_json::from_value(json).unwrap();
    assert_eq!(back.field_name, "value");
}
```

Always test both the serialised field name (camelCase, from `#[serde(rename)]`) and the deserialised Rust field name (snake_case).

---

## Freshness test

After any change to `chromist-pdl` or the PDL files:

```sh
cargo xtask codegen
cargo test -p chromist-cdp --test generate generated_code_is_fresh
```

The freshness test fails if `cdp.rs` was not regenerated after generator changes. It is read-only — it never rewrites the file.
