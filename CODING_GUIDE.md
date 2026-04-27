# Coding Guide — chromist

Rules for writing code that earns an **A** on the [GRADING_GUIDE](GRADING_GUIDE.md) rubric. Each section maps to one grading axis (5 universal + 4 context). Follow these rules by default; deviate only when you have a written reason.

The guide is prescriptive, not aspirational. Every rule here is either already in the codebase (with a pointer) or is the direct fix for a known gap.

---

## Universal Axes

### 1. Correctness & Testing

**Non-substitutable.** A `fair` here caps the whole crate at B-.

**Rules**
- Every public function has at least one test. Every non-trivial branch has a test that exercises it.
- Use the right test tier: unit tests for logic, integration tests across module boundaries, property tests for parsers / codecs / state machines. The `chromist-pdl` parser is a natural candidate for property tests on round-tripping.
- For generated code, add a freshness test. `cargo test -p chromist-cdp --test generate generated_code_is_fresh` is the reference — it fails if `cdp.rs` is out of date. Any new generator output must have an equivalent check.
- Test error paths, not just happy paths. `CommandDispatcher` tests (`crates/chromist/src/dispatch.rs`) cover timeout, double-send, and connection-drop — mirror this pattern for new components.
- Assertions must assert something meaningful. Constructing a value and comparing it to itself is not coverage.

**Don't**
- `#[ignore]` on a failing test without a tracking issue.
- Add a module with no test file under `tests/` or a `#[cfg(test)] mod tests` block.
- Ship a parser or codec with hand-picked examples only — add a property test.

---

### 2. Safety & Soundness

**Non-substitutable.** A `fair` here caps the whole crate at B-.

**Rules**
- Every `unsafe` block requires a `// SAFETY:` comment naming the invariant upheld and where it is established. `crates/chromist/src/browser/mod.rs:137–188` is the reference.
- Scope `unsafe` to the narrowest block, not the whole function. A safe wrapper plus a tight internal `unsafe` block is correct; `unsafe fn foo() { /* 100 lines */ }` is not.
- Handle lock poisoning explicitly. Every `rwlock.read()` / `rwlock.write()` must `.map_err(|_| CdpError::LockPoisoned)?`, never `.unwrap()`.
- Decoded base64 bytes from the browser (`chromist_types::Binary`) are untrusted input. Do not interpret them as UTF-8 or execute them without validation.
- User-supplied strings never reach `Command::arg` directly. Allowlist accepted Chrome flags or sanitise before constructing arguments.
- File paths from user input are canonicalised and bounds-checked against a root before use.
- `unwrap` / `expect` only on values whose invariant is locally obvious (e.g. immediately after a successful `Result::Ok` pattern match). Otherwise return `Result`.

**Don't**
- `unsafe { /* no comment */ }`.
- `unwrap()` on `base64::decode`, `RwLock::read/write`, or any fallible operation that takes arbitrary input.
- `format!("{user_input}")` into a subprocess argument.

---

### 3. Efficiency

**Rules**
- Use `Cow<'static, str>` for method identifiers. Generated code already does this via `chromist_types::MethodId`; maintain it in handwritten command dispatch.
- Use `Arc<str>` for session IDs. Every `HandlerMessage` variant carries `Option<Arc<str>>` (`crates/chromist/src/cmd.rs:23–37`) — clone the `Arc`, not the string.
- Release `RwLock` guards before any `.await`. Pattern: `{ let v = lock.read()?; v.clone() }`, drop guard, then `.await`.
- Use `FnvHashMap` for hot-path maps keyed by integer-like types. `dispatch.rs` is the reference.
- Share event frames via `Arc<EventFrame>`. Fan-out to N subscribers clones the `Arc` (one atomic increment), never the payload.
- Use enum dispatch for transport polymorphism (`AnyConnection` in `conn.rs:76`), not `Box<dyn Trait>`.
- Box enum variants larger than two pointer widths. Examples: `CdpError::WebSocket(Box<tungstenite::Error>)`, `CdpError::JavascriptException(Box<ExceptionDetails>)`.
- Bound all event channels. Use `EVENT_CHANNEL_CAP` (512). When a subscriber's channel is full, drop the event — not the subscriber.
- Prune caches on destruction events. `handler/mod.rs` removes entries on `Target.targetDestroyed`; any new cache needs an equivalent removal path.
- **Benchmark every hot path.** Any code that runs at event-rate, per-command, or in a parser / codec loop must have a `criterion` bench in the crate's `benches/` directory. Internal types reach benches through a gated `_bench` feature that re-exports them under a `__bench` module — never widen public visibility for a bench. `crates/chromist/benches/event_fanout.rs` is the reference; it sweeps subscriber counts (0, 1, 4, 16, 64, 256) and covers global / session / mixed / disconnected-pruning paths. Run with `cargo bench -p chromist --features _bench`.

**Don't**
- `BTreeMap` for the dispatch map — O(log n) vs O(1) matters at thousands of concurrent requests.
- Clone `serde_json::Value` blobs inside the event dispatch loop (current gap in `listeners.rs:81`).
- `String::clone` for session IDs on each message dispatch.
- Unbounded `mpsc::channel` for event fan-out.
- Retain destroyed targets in `target_cache` — unbounded growth across a long session.

---

### 4. Maintainability

**Rules**
- `pub(crate)` for internal plumbing. `CommandDispatcher`, `EventBus`, the `dispatch` and `event_bus` modules are `pub(crate)` in `lib.rs`. Keep internal types internal.
- Name magic values. `EVENT_CHANNEL_CAP`, `DEFAULT_REQUEST_TIMEOUT`, `SWEEP_INTERVAL` are named constants, not inline literals.
- No sentinel strings. Use proper enum variants. `CdpType::InlineEnum(Vec<EnumVariant>)` replaced the old `"__enum__:"` string prefix — canonical example.
- Module depth ≤ 2. Any type in the `chromist` crate is reachable in at most two hops from `lib.rs`.
- Prefer iterators over index loops. Prefer ownership over `Rc` / gratuitous `Clone`. Lifetimes where they clarify; generics where they don't obfuscate.
- Rustdoc on every `pub` item. Non-trivial public APIs include runnable doc-tests.
- Delete dead code. Unused `pub` items, commented-out logic, `_unused` variables that have outlived their purpose.

**Don't**
- Re-export types that are not part of the public API with bare `pub`.
- Speculative generics or traits with a single implementation — wait until the second impl is needed.
- Three-level nested `match` — extract a helper.
- `#[derive(Debug)]` on a type that transitively holds `Vec<serde_json::Value>` without a manual `fmt::Debug` using `finish_non_exhaustive()`.

---

### 5. Robustness

**Rules**
- Every distinct failure mode gets a named `CdpError` variant. Do not add `Launch(String)` catch-alls — define `LaunchExit { exit_status, stderr }`, `LaunchTimeout { stderr }`, etc.
- Call `fail_all_closed()` on connection teardown, every time. Any code path that breaks the WebSocket or pipe connection drains the dispatcher so no caller blocks forever.
- Capture the raw payload on `InvalidMessage`. Callers may choose to swallow the error (`ignore_invalid_messages`), but must log at `warn!` before doing so.
- Clean up resources on drop: tempdirs removed, file handles closed, child processes killed. Log failures at `warn!` — don't silently swallow.
- When Chrome dies mid-session, the `conn.next()` arm returns `None`. That path calls `fail_all_closed()` and breaks — it does not silently loop.
- Long-running operations accept a deadline. `NavigationWaiter` takes a timeout; new wait APIs follow the same pattern.

**Don't**
- `if let Ok(x) = fallible_op() { … }` with no `else` branch and no log — silent error swallow.
- Leave pending `oneshot::Sender`s in the dispatcher when the connection closes.
- Best-effort cleanup (tempdir removal, child kill) that discards errors without a log line.

---

## Context Axes

### 6. API Design & Stability

**Applies to:** `chromist`, `chromist-cdp`, `chromist-types` (public library crates).
**`N/A` for:** `xtask`, internal binaries, build tools with no external consumers.

**Rules**
- `#[non_exhaustive]` on every public enum. This applies especially to generated enums in `chromist-cdp` (every CDP event enum, every variant enum). Without it, adding a new CDP variant is a breaking change for downstream `match` expressions. The generator (`crates/chromist-pdl/src/build/generator.rs`, `event.rs`) emits this; maintain that.
- Deprecate before removing. Any `pub` item being retired gets `#[deprecated(since = "x.y.z", note = "use Foo instead")]` for at least one minor version before deletion.
- Keep `pub(crate)` on internal plumbing. Review `pub` items in each PR.
- Generated identifiers are part of the API. Field names in `chromist-cdp` derive from CDP PDL camelCase → Rust snake_case. Do not rename them in the generator without a deprecation cycle.
- Run the freshness test (`cargo test -p chromist-cdp --test generate generated_code_is_fresh`) before every commit that touches `chromist-pdl`. Never commit a stale `cdp.rs`.
- Builder types hide their fields. Builder fields are `pub(crate)` or `pub(super)`; construction goes through the methods.

**Don't**
- Add a public enum variant without checking whether `#[non_exhaustive]` is on the enum.
- Remove a deprecated item in the same PR that adds the deprecation annotation.
- Change a generated field name (e.g. `frame_id` → `frame_id_str`) without a migration path.

---

### 7. Concurrency Correctness

**Applies to:** `chromist` (async I/O, shared caches, event fan-out).
**`N/A` for:** `chromist-types` (pure data), `chromist-pdl` (single-threaded codegen), `chromist-cdp` (generated wire types).

**Rules**
- **Async iff the method does I/O or might block.** Sync for cached state, handle construction, and pure computation. Async signatures advertise "I might suspend"; if a method does no I/O, the `.await` is a lie that taxes every caller (executor overhead, contagious async, lost iterator/`?` ergonomics, can't call from `Drop` or `Display`). Reference: `Frame::url()` and `Page::url()` are sync (read cached `frame_tree`); `Page::title()` and `Page::cookies()` are async (CDP round-trip). The asymmetry is deliberate — readers can tell which methods cost a wire call from the call site.
- Never `.await` while holding a `std::sync` lock. Acquire, extract (cloning if necessary), drop the guard, then `.await`. Audit manually — the compiler does not enforce this.
- All `select!` arms are cancel-safe. `mpsc::Receiver::next()` is cancel-safe (items re-queued). `sleep().fuse()` is cancel-safe. `conn.next()` — verify the underlying stream implementation.
- Re-arm the tick future by reassignment inside its arm:
  ```rust
  _ = tick => {
      self.dispatch.sweep_timeouts();
      tick = Box::pin(crate::runtime::sleep(SWEEP_INTERVAL).fuse());
  }
  ```
  Reusing a `Fuse` after it fires disables the arm permanently.
- `resolve()` removes from the map before sending. `CommandDispatcher::resolve` removes the entry, then sends — prevents double-delivery if a sweep races with a late response.
- Update the cache before publishing the event. The invariant in `handler/mod.rs:350–375` (`handle_incoming` → `apply_to_tree` runs before `bus.publish(frame)`). Never invert this order.
- Drop ordering — background tasks clean up when their owners drop. No pending callers left hanging.
- Atomic memory ordering is chosen deliberately. `Acquire` / `Release` pair on `Arc<AtomicBool>` flags (`page/mod.rs:105, 148`); `Relaxed` only for counters with no happens-before dependency.

**Don't**
- Make a method `async fn` for "uniformity" with sibling methods that legitimately do I/O. Forcing sync work through the async machinery costs every caller and infects the call graph upward.
- Make a method `async fn` to "future-proof" against later adding I/O. Both directions of the sync↔async migration are breaking changes; start sync and convert later if I/O is genuinely needed.
- `lock.read().unwrap()` inside an `async fn` that also awaits — guard held across `.await`.
- Create the tick future inside the loop body — this resets the timer on every inbound message.
- Call `bus.publish` before `update_target_cache` for target lifecycle events.
- Spawn unbounded background tasks (one per session, per page, per connection) with no pool or shared listener.

---

### 8. Observability

**Applies to:** `chromist` (runtime library).
**`N/A` for:** `chromist-types` (pure data), `chromist-pdl` (build tool — use `eprintln!` in the build script is fine), `chromist-cdp` (generated).

**Rules**
- Instrument public entry points with `tracing` spans. `HandlerHandle::execute` uses `#[tracing::instrument(skip_all, fields(method = %cmd.identifier()))]` (`handler/mod.rs:169`). `Browser::launch`, `Page::attach`, `HandlerHandle::subscribe` follow the same pattern.
- Per-frame / per-event logs at `tracing::trace!` only. `debug!` and above must not appear inside the main dispatch loop — at scale, debug-level frame logs flood output and are disabled in production anyway.
- Lifecycle events at `debug!` or `info!`. `browser/mod.rs:299` logs `tracing::info!(ws_url, "browser launched successfully")`; launch failures at `debug!`.
- Structured errors, not string payloads. Prefer `CdpError::LaunchExit { exit_status, stderr }` over `CdpError::Launch("exited: …".to_string())`. Fields are filterable in log aggregators; embedded strings are not.
- Silent drops are logged. `EventStream::poll_next` logs deserialisation failures at `trace!` (`listeners.rs:84–88`). Full channels, poisoned locks, and malformed frames all get at least a `trace!` log before being dropped.
- `Debug` impls on types that may hold large `serde_json::Value` fields use `finish_non_exhaustive()` or a manual impl that truncates.
- No `eprintln!` / `println!` in library code. All output flows through `tracing`. Callers choose their subscriber.

**Don't**
- `CdpError::Launch(format!("…{e}"))` — define a structured variant instead.
- `#[derive(Debug)]` on a type that transitively holds `Vec<serde_json::Value>` without a custom `fmt::Debug`.
- `eprintln!` for diagnostic output — it bypasses `tracing` and cannot be suppressed by the caller.

---

### 9. Build Hygiene

**Applies to:** every crate in the workspace.

**Rules**
- Clippy-clean. No blanket `#[allow(clippy::all)]` except on generated code, and even there scope to specific lint categories (e.g. `clippy::large_enum_variant`, `clippy::too_many_arguments`).
- Warning-free build. No dead-code, unused-import, or deprecated warnings. Fix them in the PR that introduces them.
- `rust-version` declared in `Cargo.toml` for all workspace crates. Bump it deliberately — don't let MSRV drift silently via dependency updates.
- Feature flags are additive. Defaults are documented in the crate docs. Optional dependencies are gated by the feature that enables them.
- Dependency versions are consistent across the workspace — one `syn` major version, one `serde` major version. The workspace `Cargo.toml` is the reference.
- Compile time is understood. `chromist-cdp`'s `cdp.rs` is a known large file; it exists as a single `include!` target because the linker overhead of hundreds of small files outweighs the incremental-compile gain.

**Don't**
- `#[allow(clippy::all)]` at the crate root on hand-written code.
- Add a dependency without checking whether another workspace crate already depends on an alternative (two JSON libraries, two hashmap libraries, etc.).
- Activate optional dependencies by default.
- Commit with `cargo build --workspace` producing warnings.

See [CLAUDE.md](CLAUDE.md) — "CI gates" — for the four-command local checklist (`cargo check` / `fmt --check` / `clippy -D warnings` / `test`) that mirrors CI exactly.

---

## Quick Checklist

Run through this before opening a PR:

| # | Check |
|---|-------|
| 1 | Every new public function has at least one test |
| 2 | Every new `unsafe` block has a `// SAFETY:` comment |
| 3 | No `std::sync` lock held across `.await` |
| 4 | All new `pub enum` types carry `#[non_exhaustive]` |
| 5 | New event channels use `mpsc::channel(EVENT_CHANNEL_CAP)`, not unbounded |
| 6 | Every `rwlock.read/write()` call has `.map_err(|_| CdpError::LockPoisoned)?` |
| 7 | New failure modes get a named `CdpError` variant, not `Foo(String)` |
| 8 | Tick future is re-armed inside the `_ = tick =>` arm, not recreated in the loop |
| 9 | `update_target_cache` precedes `bus.publish` for target lifecycle events |
| 10 | New large enum variants are `Box`ed |
| 11 | Public entry points are `#[tracing::instrument]`-ed |
| 12 | `cargo build --workspace` produces zero warnings |
| 13 | `cargo test -p chromist-cdp --test generate generated_code_is_fresh` passes |
| 14 | Any new hot path ships with a `criterion` bench in `benches/` |
| 15 | Every new `async fn` does real I/O — no async-for-uniformity |
