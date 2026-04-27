# Grading Guide — chromist

This rubric exists so every review of this workspace — human or agent — scores each crate on the same axes with the same anchors. Apply it per-crate; compare results across reviews.

The rubric is structured around **five universal axes** that apply to every Rust crate, plus **four context axes** that are only rated when they apply. A context axis marked `N/A` is a first-class outcome — it means the axis genuinely does not apply, not that the crate scores `excellent` by default.

## Grading Scale

| Grade | Meaning |
|-------|---------|
| A+    | Ships as-is, nothing to add |
| A     | Production-ready, no real gaps |
| A-    | One non-blocking gap; fine to ship |
| B+    | Works; one real gap that should be fixed soon |
| B     | Works; two or more gaps worth a follow-up PR |
| B-    | Functional but noticeably rough in one area |
| C+    | Shippable with caveats; needs a focused cleanup |
| C     | Works under happy-path; breaks under light stress |
| D     | Incomplete or consistently unreliable |
| F     | Does not build or causes data loss |

## How to Use

For each crate, produce:
1. An **overall letter grade**
2. A per-axis rating: `excellent / good / fair / weak / N/A`
3. **2–3 "to reach the next grade"** bullets (specific, actionable, with file:line)
4. **1–2 "already strong"** bullets (concrete, not generic praise)

### Deriving the overall grade

Start from the worst *applicable* (non-N/A) rating, then apply:

| Worst applicable rating | Grade cap |
|---|---|
| All `excellent` | **A+** |
| One `good`, rest `excellent` | **A** |
| Two+ `good`, or one `fair` | **A-** |
| One `weak` | **B+** |
| Two `weak` | **B** |
| Three+ `weak`, or any `fair`/`weak` on a **non-substitutable** axis | **B-** |

**Non-substitutable axes:** `Correctness & Testing` and `Safety & Soundness`. A `fair` or `weak` on either caps the grade at B-, regardless of how strong everything else is. These represent the minimum bar a Rust crate must meet before any other quality dimension matters.

---

## Universal Axes

These five axes apply to every Rust crate. They cannot be `N/A`.

1. [Correctness & Testing](#1-correctness--testing)
2. [Safety & Soundness](#2-safety--soundness)
3. [Efficiency](#3-efficiency)
4. [Maintainability](#4-maintainability)
5. [Robustness](#5-robustness)

### 1. Correctness & Testing

**Non-substitutable.** Concerns whether the code does what it claims and whether tests prove it.

**What to look for:**
- Test presence — does every public function have at least one test? Every non-trivial branch?
- Test tiers — unit tests for logic, integration tests across module boundaries, property tests or fuzzing for parsers / serialisers / state machines.
- Freshness tests — for generated code, is there a test that fails if the committed output is stale? (`cargo test -p chromist-cdp --test generate generated_code_is_fresh` is the reference.)
- Meaningful assertions — tests that construct a value and then assert it equals itself are not coverage.
- Failure-mode coverage — are error paths tested, not just happy paths? Timeout, cancellation, malformed input.

**Anti-patterns:**
- Test files that exercise constructors and getters but never a real code path.
- Deleted tests left as `#[ignore]` with no tracking issue.
- Property-testable logic (parsers, codecs, state machines) with only hand-written examples.

---

### 2. Safety & Soundness

**Non-substitutable.** Concerns `unsafe` correctness, trust boundaries, and panic discipline.

**What to look for:**
- Every `unsafe` block carries a `// SAFETY:` comment naming the invariant upheld and where it is established.
- `unsafe` scope is minimal — block scope, not function scope. No `unsafe fn` where a safe wrapper plus narrow internal `unsafe` block would suffice.
- Miri-clean (where applicable — pointer / FFI code in particular).
- Input validation at trust boundaries: user input, network input, file paths, deserialised data.
- Panic discipline: `unwrap` / `expect` only on values whose invariant is locally obvious; otherwise return `Result`.
- Subprocess arguments — no unsanitised user input reaches `Command::arg` / `args`.
- Path traversal — user-supplied paths are canonicalised and bounds-checked before use.

**Anti-patterns:**
- `unsafe { /* no comment */ }`.
- `unwrap()` on `RwLock::read/write` — lock poisoning surfaces as a panic instead of an error.
- `expect("unreachable")` inside a function that takes arbitrary input.
- Passing user-supplied strings directly as CLI flags or shell arguments.

---

### 3. Efficiency

Concerns latency, throughput, and memory footprint, evaluated against the crate's realistic load — not hypothetical scale.

**What to look for:**
- Hot-path allocations — does the main loop allocate a fresh `Vec` / `String` / `HashMap` per call?
- Lock-held durations — are locks held only long enough to extract a value, not across I/O or `.await`?
- Algorithmic complexity — `FnvHashMap` (O(1)) vs `BTreeMap` (O(log n)) on hot paths; `O(n²)` scans that could be `O(n)`.
- Sharing vs cloning — `Arc<T>` fan-out for large values shared across many consumers.
- Large enum variants boxed — variants holding 3+ fields or types exceeding `2 * size_of::<usize>()` wrapped in `Box`.
- Channel discipline — bounded channels for event streams with an explicit drop policy; unbounded only for control messages.
- Compile-time cost — a single massive `include!`d file is fine; hundreds of small generated files slows incremental builds.
- **Bottlenecks are benchmarked.** Every identified hot path — event fan-out, command dispatch, parser, codec, cache lookup — has a `criterion` benchmark in `benches/` that covers realistic input shapes and subscriber / concurrency counts. The bench makes the rating *quantitative*: a regression shows up as wall-clock, not a gut call. Reference: `crates/chromist/benches/event_fanout.rs`.

**Rating anchors:**
- `excellent` — all known hot paths have benches; allocations and locks are scrutinised; no obvious waste.
- `good` — hot paths are clean but one or two are unbenched; regressions would go unnoticed until symptoms appear.
- `fair` — a hot path allocates or clones unnecessarily, or there are zero benches for a crate where one is obviously warranted (async I/O, parser, codec).
- `weak` — quadratic behaviour, unbounded growth, or lock-across-`.await` on a path that runs at event-rate.

**Anti-patterns:**
- Cloning `serde_json::Value` blobs inside a dispatch loop.
- Unbounded `mpsc` channel for high-frequency events with no backpressure.
- `String::clone` for identifiers that could be `Arc<str>`.
- Retaining destroyed entries in a cache indefinitely.
- A performance fix landing without a bench that would have caught the regression.

---

### 4. Maintainability

Concerns cognitive load, idiom, and documentation — how quickly a new contributor can understand, change, and navigate the crate.

**What to look for:**
- Idiomatic Rust — iterators over index loops, ownership over gratuitous `Rc`/`Clone`, lifetimes where they clarify, generics where they don't obfuscate.
- Module depth — any type reachable in ≤ 2 hops from `lib.rs`.
- Public surface — `pub(crate)` on internal plumbing; `pub` only on types downstream code should touch.
- Named constants — magic numbers replaced by named `const`s with non-obvious meanings documented.
- Rustdoc coverage — public items carry doc comments; non-trivial ones include runnable examples (doc-tests).
- Dead code — no unused `pub` items, commented-out logic, or `_unused` variables that have outlived their purpose.
- Speculative abstraction — no traits with one impl, no generics waiting for a second caller that may never arrive.

**Anti-patterns:**
- Sentinel strings like `"__enum__:"` prefixes where a proper enum would work.
- Deeply nested `match` arms where a helper would flatten the structure.
- `pub` re-exports of internal plumbing.
- `#[derive(Debug)]` on types that transitively hold multi-kilobyte payloads.

---

### 5. Robustness

Concerns correct behaviour under failure: I/O errors, connection drops, partial inputs, resource exhaustion, cancellation.

**What to look for:**
- Every distinct failure mode has a named error variant — no `Foo(String)` catch-alls.
- Resource cleanup on drop — tempdirs removed, file handles closed, child processes killed.
- Teardown completeness — when the main loop exits, do all pending callers receive a definitive error (not hang)?
- Timeout and cancellation discipline — long-running operations accept a deadline or cancel token.
- Lock poisoning handled — every `RwLock::read/write` converts poison to a typed error, never panics.
- Malformed input surfaces diagnostic context — the raw payload is captured in the error, not dropped.

**Anti-patterns:**
- Silent error swallowing — `if let Ok(x) = result { … }` with no else branch and no log.
- Leaving pending `oneshot::Sender`s in a dispatcher when the connection drops.
- `panic!` / `unwrap()` on `RwLock` operations in production code paths.
- Best-effort cleanup with no log on failure (tempdirs, PID files, sockets).

---

## Context Axes

These four axes are only rated when they apply. Mark `N/A` otherwise — do not default to `excellent`.

6. [API Design & Stability](#6-api-design--stability)
7. [Concurrency Correctness](#7-concurrency-correctness)
8. [Observability](#8-observability)
9. [Build Hygiene](#9-build-hygiene)

### 6. API Design & Stability

**Applies when:** the crate is published or consumed by external code (library crates, workspace-public crates).
**`N/A` when:** the crate is an internal binary, build tool, or test helper with no external consumers.

**What to look for:**
- `#[non_exhaustive]` on every public enum — without it, adding a variant is a breaking change.
- Builder pattern for types with optional fields; builder fields not `pub`.
- Semver discipline — breaking changes follow a deprecation cycle.
- Generated identifier stability — field / variant names are part of the API surface.
- `pub` vs `pub(crate)` — internal plumbing is not leaked.
- Regeneration determinism — for generated code, byte-identical output across runs.

**Anti-patterns:**
- `pub enum Foo { A, B }` without `#[non_exhaustive]` — callers write exhaustive matches, you can never add `C`.
- Renaming a generated field without a deprecation cycle.
- Exposing `HandlerHandle` / internal state fields as `pub`.

---

### 7. Concurrency Correctness

**Applies when:** the crate uses `async`, threads, or shared mutable state across boundaries.
**`N/A` when:** the crate is synchronous and single-threaded (parsers, pure data types, build tools).

**What to look for:**
- No `.await` while holding a `std::sync` lock.
- Cancel-safety of every `select!` arm — dropping the future mid-poll must not lose data.
- `Fuse`d futures in `select!` loops are re-armed by reassignment inside their arm, not recreated on every iteration.
- Ordering invariants — cache writes precede event publishes, map removal precedes channel send to prevent double-delivery.
- Drop ordering — background tasks clean up when their owners drop; no pending callers left hanging.
- Atomic memory ordering is justified — `Acquire` / `Release` chosen deliberately, not `Relaxed` by default.

**Anti-patterns:**
- `lock.write().unwrap().field = …; other.await;` — guard held across await.
- Reusing a `Fuse` after it fires — the arm becomes permanently disabled.
- Publishing an event before updating the cache subscribers will read.
- Unbounded background task spawns (one per session, per connection, per page) with no pool or shared listener.

---

### 8. Observability

**Applies when:** the crate runs in production, is a long-running service, or is a library whose failures need diagnosing remotely.
**`N/A` when:** the crate is a build tool, proc-macro, or pure-data type where failures surface at compile time.

**What to look for:**
- `tracing` spans on public entry points — `#[instrument]` or manual `debug_span!`.
- Structured error fields — `CdpError::LaunchExit { exit_status, stderr }` instead of `Launch(String)`.
- Log-level discipline — per-event / per-frame logs at `trace!`, lifecycle events at `debug!` or `info!`, failures at `warn!` or above.
- `Debug` impls on large types use `finish_non_exhaustive()` rather than dumping multi-kilobyte payloads.
- Silent drops (failed deserialisation, full channels, poisoned locks) are logged, even if the user-facing error is swallowed.
- No `eprintln!` / `println!` in library code — all output flows through `tracing`.

**Anti-patterns:**
- No `tracing` dependency at all — failures are invisible without attaching a debugger.
- `#[derive(Debug)]` on a type holding `Vec<serde_json::Value>`.
- `if let Ok(x) = decode(frame) { … }` with no `else` branch and no log.

---

### 9. Build Hygiene

**Applies when:** the crate is part of a workspace or library distribution.
**`N/A`:** rarely — almost every crate benefits from this axis. Mark `N/A` only for pure scratch / experiment crates.

**What to look for:**
- Clippy-clean — no blanket `#[allow(clippy::all)]` except on generated code, and even there scoped to specific lint categories.
- Warning-free build — no dead-code, unused-import, or deprecated warnings.
- MSRV policy documented in `Cargo.toml` (`rust-version = "…"`).
- Feature-flag hygiene — features are additive, default features are documented, optional dependencies gated.
- Dependency discipline — dependency count is justified, versions are consistent across the workspace, no known-vulnerable crates.
- Compile time — the crate builds in seconds to low-tens-of-seconds; if not, the cost is understood and documented.

**Anti-patterns:**
- `#[allow(clippy::all)]` at the crate root on hand-written code.
- `rust-version` missing from `Cargo.toml`.
- Duplicate versions of the same dependency across the workspace (e.g. two `syn` major versions).
- Optional dependencies activated by default.

---

## Worked Example — `chromist` (main crate)

**Overall: A-**

| Axis | Rating |
|------|--------|
| Correctness & Testing | good |
| Safety & Soundness | excellent |
| Efficiency | fair |
| Maintainability | excellent |
| Robustness | good |
| API Design & Stability | good |
| Concurrency Correctness | good |
| Observability | good |
| Build Hygiene | good |

**Already strong:**
- `Arc<EventFrame>` + bounded channels (`crates/chromist/src/cmd.rs:17` — `EVENT_CHANNEL_CAP = 512`) give zero-copy fan-out with automatic backpressure drop; slow subscribers don't stall others.
- `handler/mod.rs` updates `target_cache` before calling `EventBus::publish` (`crates/chromist/src/handler/mod.rs:295–303`), so no subscriber ever observes a stale target list — this ordering invariant is documented in the module comment.

**To reach A:**
- `listeners.rs:81` — `EventStream::poll_next` clones the entire `frame.params: Value` on every matching event. Pass the `Arc<EventFrame>` through and deserialise from it, rather than cloning the `Value`.
- `network.rs:256–265` — `HttpRequest` is fully cloned on every response update. Hold mutable state behind an `Arc<Mutex<>>` so fan-out shares the allocation.
- `page/mod.rs:96–110` — one background watcher task per `Page::attach`. Drive all page-destroyed events through a shared multiplexed listener.

---

## Non-Goals

- Not a security audit — use `/security-review` for that.
- Not a style guide — defer to `rustfmt` and `clippy` lints.
- Not a performance budget. The Efficiency axis asks that benches *exist* for hot paths; it does not set ns/op thresholds. Use the benches to catch regressions over time, not as absolute pass/fail gates.
