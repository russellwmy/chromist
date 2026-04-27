# Benchmarks

Microbenchmarks for the chromist runtime hot paths: command-response correlation
(`CommandDispatcher`), event fan-out (`EventBus`), and typed-event filtering
(`EventStream`). Numbers are wall-clock medians from Criterion (`time:` row).

Run locally with:

```sh
cargo bench -p chromist --features _bench --bench dispatch
cargo bench -p chromist --features _bench --bench event_fanout
cargo bench -p chromist --features _bench --bench event_stream
```

The `_bench` feature is internal — it `pub`-exposes `CommandDispatcher`,
`EventBus`, and `EventFrame` so the benches in `crates/chromist/benches/` can
reach them without going through the public API. Not part of the published
surface.

---

## Reference run

| Field | Value |
|---|---|
| Date | 2026-04-27 |
| Host | Apple M1 Max (arm64) |
| OS | macOS 26.3.1 |
| Toolchain | rustc 1.95.0 (stable, 2026-04-14) |
| Profile | `cargo bench` (release + debug-assertions off) |

Re-run on different hardware before drawing absolute conclusions; the shapes
(O(n) sweeps, sub-µs single-element ops) are what's load-bearing.

---

## CommandDispatcher (`dispatch.rs`)

Request-response correlation. `register` adds a oneshot to the in-flight map;
`resolve` removes the matching entry and sends. `sweep_*` is the periodic
500 ms timeout pass.

### `register` — add a pending command

| In-flight count | Median |
|---:|---:|
| 0 | 72 ns |
| 16 | 522 ns |
| 256 | 7.21 µs |
| 4096 | 118.09 µs |

Linear growth confirms the FnvHashMap insert is the dominant term. Per-call
amortised: ~30 ns at any size.

### `resolve` — fulfil a oneshot

| In-flight count | Median |
|---:|---:|
| 1 | 65 ns |
| 16 | 536 ns |
| 256 | 7.29 µs |
| 4096 | 117.79 µs |

Same shape as register; the work is symmetric (lookup + remove + send vs
insert).

### `sweep_timeouts` — happy path (no expired entries)

| In-flight count | Median |
|---:|---:|
| 0 | 19 ns |
| 16 | 66 ns |
| 256 | 946 ns |
| 4096 | 17.67 µs |

This is the cost the handler eats every 500 ms, every tick. At 4096
in-flight commands it's still ~18 µs — call it 0.0036 % CPU at the
default tick rate. **No reason to raise the sweep interval.**

### `sweep_timeouts` — every entry expired

| In-flight count | Median |
|---:|---:|
| 1 | 97 ns |
| 16 | 744 ns |
| 256 | 11.30 µs |
| 4096 | 182.66 µs |

Worst case — every entry timed out simultaneously. Still ~10× the no-expired
cost, dominated by error-construction + send overhead.

---

## EventBus (`event_bus.rs`)

`publish` fans an `Arc<EventFrame>` to all matching subscribers. Bounded
channels at `EVENT_CHANNEL_CAP = 512`; full channels drop the event, not the
subscriber.

### Fan-out — global subscribers (one event)

| Subscribers | Median |
|---:|---:|
| 0 | 11 ns |
| 1 | 38 ns |
| 4 | 124 ns |
| 16 | 479 ns |
| 64 | 1.89 µs |
| 256 | 7.60 µs |

~30 ns per subscriber. The Arc clone + non-blocking send is the floor.

### Fan-out — session-scoped subscribers (one event)

| Subscribers | Median |
|---:|---:|
| 1 | 53 ns |
| 8 | 77 ns |
| 64 | 241 ns |
| 256 | 699 ns |

Cheaper than global because the session lookup short-circuits non-matching
subscribers — only the entries with a matching `session_id` get the send.

### Fan-out — mixed (50/50 global + session)

| Subscribers | Median |
|---:|---:|
| 1 | 167 ns |
| 8 | 241 ns |
| 32 | 424 ns |
| 128 | 1.13 µs |

### `prune_dead` — drop disconnected receivers

| Subscribers | Median |
|---:|---:|
| 1 | 411 ns |
| 16 | 1.18 µs |
| 128 | 6.78 µs |

Called lazily during publish when a send hits a closed receiver.

---

## EventStream (`listeners.rs`)

`EventStream<T>::poll_next` filters incoming `Arc<EventFrame>` by method id
and deserialises matching frames into `T`. Two cases: every frame matches
(deserialise hot path), every frame is for a different method (skip hot
path).

### `all_match` — every frame deserialises

| Burst size | Median |
|---:|---:|
| 1 | 316 ns |
| 8 | 1.16 µs |
| 64 | 8.08 µs |
| 256 | 35.74 µs |

~140 ns/frame is dominated by the `serde_json::Value` → `T` step.

### `all_skip` — every frame filters out

| Burst size | Median |
|---:|---:|
| 1 | 288 ns |
| 8 | 1.07 µs |
| 64 | 7.38 µs |
| 256 | 32.93 µs |

The method-id `Arc<str>` comparison is the dominant cost on the skip path
because the channel poll itself is non-zero.

---

## Notes for re-running

- Benches run with `--features _bench` to expose internals.
- Criterion's HTML output lands under `target/criterion/`.
- Re-run a single bench: `cargo bench -p chromist --features _bench --bench dispatch -- "dispatch/register/256"`.
- For CI gating use the `change:` row Criterion prints — Criterion will fail
  the run on its own threshold (`p < 0.05`) if you wire `--save-baseline`
  + `--baseline` into a CI job.
