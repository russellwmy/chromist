# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
# Build
cargo build --workspace

# Test everything
cargo test --workspace

# Test a single crate
cargo test -p chromist-types
cargo test -p chromist-pdl
cargo test -p chromist-cdp
cargo test -p chromist

# Run a single test by name
cargo test -p chromist dispatch::tests::test_register_and_resolve_success

# Regenerate cdp.rs after editing chromist-pdl or updating PDL files
cargo xtask codegen

# Verify cdp.rs is fresh (run after any chromist-pdl change)
cargo test -p chromist-cdp --test generate generated_code_is_fresh

# Check without building artefacts
cargo check --workspace

# Benchmarks (require the internal `_bench` feature)
cargo bench -p chromist --features _bench --bench event_fanout
cargo bench -p chromist --features _bench --bench dispatch
```

## CI gates — run before declaring any task complete

CI (`.github/workflows/ci.yml`) gates every PR on the `lint` job. To avoid the back-and-forth of pushing a green local build only to have CI catch a clippy lint or fmt diff, run **all four** of these locally before reporting a task done. They mirror the CI `lint` and `test` jobs exactly:

```sh
cargo check --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If `cargo fmt --all --check` reports diffs, run `cargo fmt --all` and re-run the check. If `cargo clippy` flags a lint, fix the underlying issue — do not add `#[allow(...)]` to silence it unless the lint is genuinely wrong for the call site (and document why). Common foot-guns this catches:

- `clippy::items-after-test-module` — non-test items declared *after* a `#[cfg(test)] mod …` block. Always place test-gated modules at the end of the file.
- `clippy::useless-conversion` — `.into()` on a value that already has the target type. Common after refactors that change a return type from `&str` to `Arc<str>`.
- `clippy::needless-borrow`, `clippy::redundant-clone`, `clippy::unused-async` — show up after refactors that change ownership or remove `.await`.

The `integration` and `deny` CI jobs are not part of the local fast-loop (they require a Chromium binary and `cargo-deny` respectively), but if you're touching dependency declarations, also run `cargo deny check` locally if available.

## Generated file

`crates/chromist-cdp/src/cdp.rs` is **fully generated** — never edit it by hand. The source of truth is the PDL files under `crates/chromist-cdp/pdl/`. After any change to `chromist-pdl` or the PDL files, run `cargo xtask codegen` and commit the result. The freshness test enforces this.

## Crate dependency order

```
chromist-types   ← wire types, traits, BuildError (no I/O)
chromist-pdl     ← PDL parser + code generator (build-time only, not used at runtime)
chromist-cdp     ← pre-generated CDP bindings (one module per domain)
chromist         ← high-level public API: Browser, Page, Handler, Element
xtask            ← developer task runner (publish = false)
```

Dependencies only flow downward. `chromist-types` is the only crate all others share.

## Handler loop architecture

One `Handler` background task owns the WebSocket/pipe connection. Callers interact through `HandlerHandle` (cheaply cloneable — holds only an unbounded channel sender and an `Arc<RwLock<target_cache>>`).

The `Handler::run` loop races three branches via `futures::select!`:
1. Inbound message from the browser → `CommandDispatcher::resolve` (response) or `EventBus::publish` (event)
2. Inbound message from a caller → `conn.submit_command` + register in dispatcher
3. 500 ms sweep tick → `CommandDispatcher::sweep_timeouts`

**Critical invariant:** the tick future is created once before the loop and re-armed by reassignment inside its arm — recreating it inside the loop body resets the 500 ms clock on every message.

**Critical invariant:** `update_target_cache` runs before `EventBus::publish` for every target lifecycle event. Subscribers must always see an up-to-date cache when they react to `Target.*` events.

## Request-response correlation

`CommandDispatcher` (`dispatch.rs`) uses `FnvHashMap<CallId, (oneshot::Sender, Instant)>`. The registration timestamp lives alongside the sender so `resolve()` removing an entry also removes it — no stale entries. `sweep_timeouts()` computes elapsed time via `saturating_duration_since` against `request_timeout` and is O(n) in active calls. `resolve()` logs round-trip latency at `tracing::debug!` level.

## Event fan-out

`EventBus` fans out `Arc<EventFrame>` to bounded channels (`EVENT_CHANNEL_CAP = 512` in `cmd.rs`). Full channels drop the event, not the subscriber. `EventStream<T>` filters by `T::method_id()` and deserializes on the consumer side.

## Page submodule layout

`Page` methods are split across `impl Page` blocks in topical files under `crates/chromist/src/page/`: `navigation.rs`, `content.rs`, `evaluation.rs`, `dom.rs`, `emulation.rs`, `cookies.rs`, `capture.rs`, `input.rs`, `intercept.rs`. Fields use `pub(in crate::page)` so submodules share state without exposing it to the rest of the crate.

`NavigationWaiter` must be created *before* calling `goto` to avoid a race where the lifecycle event fires before the subscription is registered.

## Transport

`AnyConnection` is an enum (not a trait object) over `Connection<T>` (WebSocket) and `PipeConnection<T>` (Unix OS pipe). The WebSocket variant drains `pending_send` before reading on each `poll_next` — CDP requires commands to be sent promptly, and reads must not starve sends.

## Error handling

`CdpError` (`error.rs`) uses `thiserror`. Add a new named variant for each distinct failure mode — do not widen `Launch(String)`. Large variants are `Box`ed (e.g. `WebSocket(Box<tungstenite::Error>)`, `JavascriptException(Box<ExceptionDetails>)`).

Every `RwLock::read/write()` call must use `.map_err(|_| CdpError::LockPoisoned)?` — never `.unwrap()`.

## Grading report trigger

When the user asks you to **grade the project** (or grade a specific crate), after producing the grading report you **must** update the `Quality Report` section of `README.md`.

- The section is delimited by `<!-- GRADING-REPORT:START -->` and `<!-- GRADING-REPORT:END -->` markers. Replace the entire block between those markers, not the markers themselves.
- Update the `**Last graded:**` date to the current date (check `currentDate` in the system context).
- Use the rubric in `GRADING_GUIDE.md` (5 universal + 4 context axes). Do not introduce ad-hoc axes.
- If a single crate is graded, update only that crate's row in the Crate Grades table and re-derive the workspace dimension ratings from the updated set.
- **Report only the current state**, not a changelog. Never write what *changed since last grading* ("now deserialises", "replaces the previous", "improvements landed"). Changelogs belong in commits and PR descriptions, not in the README.

**Required structure** (in this order, terse — tables only, no descriptive prose):

1. **Workspace Dimensions Table** — one row per axis (all 5 universal + all 4 context axes), one column for the workspace-wide rating (`excellent / good / fair / weak / N/A`) derived from the worst applicable rating across crates, plus a one-line **Driver** note citing the crate that drives that rating. End with a bold **Overall: \<letter\>** line derived via the `GRADING_GUIDE.md` cap table.
2. **Per-Crate Sections** — one `#### <crate> — **<grade>**` subsection per crate in dependency order (`chromist-types`, `chromist-pdl`, `chromist-cdp`, `chromist`). Each subsection contains **only** a `| Axis | Rating |` table covering all 9 axes (context axes marked `N/A` when they do not apply). **No summary paragraph, no `file:line` citations, no per-crate prose** — the grade and the axis ratings speak for themselves.
3. **Improvements** — 2–5 actionable follow-ups with `file:line` citations, grouped by the grade jump they unlock (e.g. "To reach A+"). Forward-looking only. **No command invocations** (`cargo bench …`, `cargo test …`) — those belong in the Commands section of this file, not in the README's quality report.

This keeps the README's quality signal fresh and scannable. Detailed per-crate reasoning belongs in the chat turn that produced the grade, not in the README.

## Reference guides

- `ARCHITECTURE.md` — data-flow diagrams and subsystem descriptions
- `BENCHMARKS.md` — Criterion microbenchmark numbers for `dispatch`, `event_bus`, `listeners`
- `CODING_GUIDE.md` — prescriptive coding rules and PR checklist (5 universal + 4 context axes)
- `GRADING_GUIDE.md` — per-axis rubric for reviewing any crate (5 universal + 4 context axes, with N/A support)
- `TESTING_GUIDE.md` — test tiers, channel testing patterns, mock connection, where tests live
- `DEBUGGING_GUIDE.md` — common `CdpError` failure modes and diagnosis steps
- `PDL_UPDATE_GUIDE.md` — step-by-step workflow for updating vendored PDL files
- `RELEASE_GUIDE.md` — publish order, version bump rules, semver for generated bindings
