# chromist

A Rust library for controlling Chrome and Chromium over the
[Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/) (CDP).

## Overview

chromist drives a real Chromium browser from async Rust over CDP. It offers a Playwright-inspired locator and assertion API, type-safe bindings generated from the upstream protocol, and a concurrency model built around a single handler task with cheaply-cloneable handles. Scope is deliberately narrow: Tokio-only, CDP-only — no WebDriver, no Firefox.

## Telemetry

chromist sends **no telemetry, analytics, or crash reports** — there is no background phone-home, update check, or remote logging anywhere in the codebase. No third-party endpoint is hard-coded.

## Features

- Launch Chrome headless or with a visible window, or attach to a browser that's already running.
- Navigate to pages and wait for them to finish loading.
- Take screenshots and print pages to PDF.
- Run JavaScript on the page and get typed results back.
- Find elements, click, type, press keys, and scroll.
- Watch network requests, handle dialogs, and collect code coverage.
- Subscribe to browser events as async streams.
- Open isolated incognito sessions side by side.

See [CAPABILITIES.md](CAPABILITIES.md) for the full inventory.

## Usage

Add to `Cargo.toml`:

```toml
[dependencies]
chromist = "0.1"
tokio = { version = "1", features = ["full"] }
```

Requires Rust 1.95+. A Chrome or Chromium binary must be on the host; chromist auto-detects it, or set an explicit path via `BrowserConfig`.

### Quickstart

```rust
use chromist::Browser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let browser = Browser::launch_default().await?;

    let page = browser.new_page("https://example.com").await?;
    let png = page.screenshot().await?;
    std::fs::write("example.png", &png)?;
    println!("saved {} bytes", png.len());

    Ok(())
}
```

Builder for custom configuration (inside an async context):

```rust
use chromist::{Browser, BrowserConfig};

let config = BrowserConfig::builder().with_head().window_size(1920, 1080).build();
let browser = Browser::launch(config).await?;
```

### Evaluate JavaScript

```rust
let result = page.evaluate("document.title").await?;
let title: String = result.into_value()?;
println!("title: {title}");
```

### Query and interact with elements

```rust
let elements = page.dom().find_elements("a[href]").await?;
for el in elements {
    let text = el.inner_text().await?;
    println!("{text}");
}
```

### Wait for navigation

```rust
use chromist::WaitUntil;

let waiter = page.navigation_waiter(WaitUntil::NetworkIdle);
page.goto("https://example.com").await?;
waiter.wait().await?;
```

### Listen to events

```rust
use chromist::cdp::browser_protocol::network::ResponseReceivedEvent;
use futures::StreamExt;

let mut stream = page.event_listener::<ResponseReceivedEvent>();
while let Some(ev) = stream.next().await {
    println!("{}", ev.response.url);
}
```

### Attach to a running browser

```rust
// Start Chrome with: google-chrome --remote-debugging-port=9222
let browser = Browser::connect("ws://localhost:9222/json").await?;
```

### CDP Protocol Revision

Generated bindings are pinned to `chromist-cdp::PROTOCOL_REVISION`. To update, replace the PDL files under `chromist-cdp/pdl/` and run:

```sh
cargo xtask codegen
cargo test -p chromist-cdp --test generate
```

### Workspace Layout

```
crates/
  chromist-types/   # Wire types (MethodCall, Response, …) and core traits
  chromist-pdl/     # PDL parser and Rust code generator (build-time)
  chromist-cdp/     # Pre-generated CDP bindings, one module per domain
  chromist/         # High-level API: Browser, Page, Handler, Element, …
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for design details.

## Quality Report

<!-- GRADING-REPORT:START -->
**Last graded:** 2026-04-27 · **Rubric:** [GRADING_GUIDE.md](GRADING_GUIDE.md) (5 universal + 4 context axes)

### Workspace Dimensions

| Axis | Rating | Driver |
|------|--------|--------|
| Correctness & Testing | good | `chromist` — ~250 in-tree tests pass (incl. unit tests for `BrowserContext::close` + per-context `wait_for_event<T>` filtering at `context.rs:537-749`); ~12 real-browser tests gated behind `--features integration-tests` is still light for an automation library |
| Safety & Soundness | excellent | `chromist` — RAII `PipeFds` guard at `browser/launch.rs:170` closes pipe fds on `spawn()` failure; `clippy::undocumented_unsafe_blocks` + `unsafe_op_in_unsafe_fn` enforced |
| Efficiency | excellent | all crates — `BENCHMARKS.md` pins numbers for `dispatch.rs`, `event_bus.rs`, `listeners.rs`; `Arc<EventFrame>` borrow-deserialize on event hot path |
| Maintainability | excellent | `chromist` — every `pub` item documented with substantive prose (no stubs); `#![warn(missing_docs)]` + `rustdoc::broken_intra_doc_links` + `private_intra_doc_links` all enforced at `lib.rs:1-10`; single canonical path per type; `Page` god-object split into `dom()` / `emulation()` / `cookies()` / `events()` sub-handles correctly re-exported at crate root |
| Robustness | excellent | all crates — every failure mode named (incl. `CannotCloseDefaultContext` at `error.rs:159`); lock poisoning always converted via `map_err(LockPoisoned)`; `BrowserContext::close` (`context.rs:218`) disposes pages before the context; `fail_all_closed` resolves pending callers on disconnect |
| API Design & Stability | excellent | `chromist` — `BrowserContext` first-class across construction, page-flow, state, lifecycle, events, and storage_state; CDP types wrapped (`Navigation`, `DomNode`, `LayoutMetrics`, `PerfMetrics`, `InitScriptId`, `ScriptSource`); `#[non_exhaustive]` + `#[must_use]` + `#[doc(hidden)]` placed correctly |
| Concurrency Correctness | excellent | `chromist` — single Handler task; cache-write-before-publish invariant documented at `handler/mod.rs:350–375`; `select!` arms cancel-safe |
| Observability | excellent | all crates — `tracing` on entry points; large `Debug` impls use `finish_non_exhaustive()` (`Page`, `NetworkManager`, `HarRecorder`, `Connection`, `HttpRequest`); structured `LaunchTimeout { stderr }` etc. |
| Build Hygiene | excellent | all crates — clippy `-D warnings` clean; full lint header at `lib.rs:1-10` (`missing_docs`, `missing_debug_implementations`, `unreachable_pub`, `noop_method_call`, `trivial_numeric_casts`, `unsafe_op_in_unsafe_fn`, `rustdoc::broken_intra_doc_links`, `private_intra_doc_links`); MSRV 1.95.0; `deny.toml` configured |

**Overall: A**

### Per-Crate Grades

#### `chromist-types` — **A+**

| Axis | Rating |
|------|--------|
| Correctness & Testing | excellent |
| Safety & Soundness | excellent |
| Efficiency | excellent |
| Maintainability | excellent |
| Robustness | excellent |
| API Design & Stability | excellent |
| Concurrency Correctness | N/A |
| Observability | N/A |
| Build Hygiene | excellent |

#### `chromist-pdl` — **A+**

| Axis | Rating |
|------|--------|
| Correctness & Testing | excellent |
| Safety & Soundness | excellent |
| Efficiency | excellent |
| Maintainability | excellent |
| Robustness | excellent |
| API Design & Stability | excellent |
| Concurrency Correctness | N/A |
| Observability | N/A |
| Build Hygiene | excellent |

#### `chromist-cdp` — **A+**

| Axis | Rating |
|------|--------|
| Correctness & Testing | excellent |
| Safety & Soundness | excellent |
| Efficiency | excellent |
| Maintainability | excellent |
| Robustness | excellent |
| API Design & Stability | excellent |
| Concurrency Correctness | N/A |
| Observability | excellent |
| Build Hygiene | excellent |

#### `chromist` — **A**

| Axis | Rating |
|------|--------|
| Correctness & Testing | good |
| Safety & Soundness | excellent |
| Efficiency | excellent |
| Maintainability | excellent |
| Robustness | excellent |
| API Design & Stability | excellent |
| Concurrency Correctness | excellent |
| Observability | excellent |
| Build Hygiene | excellent |

### Improvements

**To reach A+** (close the only `good` rating — Correctness & Testing):

- Expand `crates/chromist/tests/integration.rs:1` from ~12 to ~30 real-browser tests covering navigation races, frame-swap, websocket interception, download error paths, and `route_from_har` round-trips. The `--features integration-tests` plumbing is in place; the gap is breadth.
- Wire the existing soak harness (`xtask/src/main.rs:53` — `xtask soak 1000`) into a nightly CI workflow that asserts ≤ +50 MiB RSS / 0 fd growth between baseline and final samples.

**Not blocking A+ but worth closing**:

- Add `cargo audit` and `cargo +nightly miri test` jobs to CI for the unsafe pipe-fd path.
- Add a `CHANGELOG.md` (keep-a-changelog format) and `CONTRIBUTING.md` (how to run tests, file layout) for governance hygiene.
<!-- GRADING-REPORT:END -->

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE).
