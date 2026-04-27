# Architecture

chromist is a 4-crate workspace. Each crate has a single responsibility, and
dependencies only flow downward — higher layers never leak implementation
details back into lower ones.

```
chromist-types   (wire protocol contracts)
      │
chromist-pdl     (build-time PDL parser + code generator)
      │
chromist-cdp     (generated CDP bindings)
      │
chromist         (high-level public API)
```

---

## Crates

### `chromist-types` — Wire Types and Traits

The foundation. Defines everything needed to speak CDP over a transport:

- **`MethodCall`** — an outbound CDP command `{ id, method, sessionId, params }`
- **`Response`** — an inbound reply `{ id, result?, error? }`
- **`CdpJsonEventMessage`** — an inbound event `{ method, params, sessionId? }`
- **`Message<T>`** — top-level untagged union that serde dispatches to either `Response` or `T`
- **`CallId`** — monotonically increasing integer correlating calls with responses
- **`Binary`** — a `Vec<u8>` newtype that serializes as base64 over the wire
- **`Method` / `MethodType` / `Command` / `EventMessage`** — traits that generated code implements

No I/O lives here. Everything is pure data and serialization logic.

### `chromist-pdl` — PDL Parser and Code Generator

A build-time crate (not used at runtime by end users) that turns Chrome's
Protocol Description Language files into Rust source.

**Pipeline:**

1. `pdl::resolver` — resolves `include <file>` directives and flattens a PDL
   document into a single string
2. `pdl::parser` — tokenizes and parses that string into an AST
   (`Protocol → Domain → TypeDef / Command / Event`)
3. `pdl::dep` — topological-sorts domains so that referenced types are always
   emitted before the types that reference them (Kahn's algorithm)
4. `build::types` — maps `CdpType` variants to `RustType` (with `Vec<_>` and
   `Box<_>` wrappers as needed for recursive references)
5. `build::builder` — emits `<Name>Builder` fluent structs for commands with
   optional parameters
6. `build::generator` — drives the whole pipeline and emits a single
   `TokenStream` string that is saved to a file for `include!`

### `chromist-cdp` — Generated CDP Bindings

Pre-generated output of `chromist-pdl`. Checked into the repository so that
end users do not need to run the generator. Contains:

- `pub mod browser_protocol` — all browser-side domains (Page, DOM, Network,
  Input, Target, …)
- `pub mod js_protocol` — JavaScript-engine domains (Runtime, Debugger, …)
- `pub mod events` — `enum CdpEvent` — a single untagged union of every event
  in both protocols, useful for generic event routing

`PROTOCOL_REVISION` records the Chromium commit position at which the PDL
files were pinned. To refresh bindings, update the PDL files and run
`cargo test -p chromist-cdp --test generate`.

### `chromist` — High-Level Public API

The main crate. Layers ergonomic abstractions over the raw CDP wire protocol.

---

## Key Subsystems

### Transport — `conn.rs` and `pipe_conn.rs`

Two transport implementations, unified under the `AnyConnection` enum:

| Variant | When used | How |
|---------|-----------|-----|
| `AnyConnection::Ws` | Default | WebSocket over TCP (`--remote-debugging-port`) |
| `AnyConnection::Pipe` | Unix only | OS pipes (`--remote-debugging-pipe`, fds 3 and 4) |

The enum avoids boxing and dynamic dispatch. Both variants implement `Stream`
and expose `submit_command`, so the `Handler` loop is transport-agnostic.

`Connection<T>` (WebSocket) drains its `pending_send` queue *before* reading
from the WebSocket on each `poll_next` call. This is intentional: CDP requires
that commands are sent promptly, and a blocked read must not starve the send
path.

In test code, a third `AnyConnection::Mock(MockConnection)` variant is available (see `conn.rs`). `MockConnection::pair()` returns a `(MockConnection, MockHandle)` — tests inject synthetic inbound messages via `MockHandle` and the handler processes them normally.

### Handler Loop — `handler/`

Module split into:
- `handler/mod.rs` — `Handler` task, `HandlerHandle`, `HandlerConfig`, `EventListeners`
- `handler/context.rs` — `BrowserContext` first-class isolation unit (per-context state: init scripts, route registry, WebSocket routes, exposed functions/bindings, geolocation, HTTP credentials, permissions, CSP-bypass, JS toggle, service-worker policy, downloads path; `close()` for lifecycle teardown)
- `handler/session.rs` — `SessionRef` live cell

A background task that owns the connection and multiplexes callers:

```
┌─────────────┐    HandlerMessage     ┌──────────────────────────┐
│  HandlerHandle │ ──────────────────► │       Handler task        │
│  (cloneable)   │                     │                          │
└─────────────┘                       │  AnyConnection           │
                                       │  CommandDispatcher       │
                                       │  EventBus                │
                                       │  TargetTree (Arc<RwLock<…>>)   │
                                       └──────────────────────────┘
```

`HandlerHandle` is cheap to clone — it holds only a channel sender and an
`Arc<RwLock<…>>` to the target cache. All actual I/O happens in the single
background task.

The main loop uses `futures::select!` to race three branches:
1. An incoming message from the connection (response or event)
2. An incoming message from a caller (`Command`, `Subscribe`, or `Shutdown`)
3. A 500 ms sweep timer to expire timed-out commands

### Request-Response Correlation — `dispatch.rs`

`CommandDispatcher` tracks every in-flight command:

- `pending: FnvHashMap<CallId, (oneshot::Sender<crate::Result<Value>>, Instant)>` — response channel and registration timestamp for each call, co-located so `resolve()` discards both at once; the timestamp also enables round-trip latency logging on resolution

When a response arrives, `resolve(id, result)` finds and fulfills the matching
`oneshot`. When the sweep timer fires, `sweep_timeouts()` is O(n) in active
calls and sends `Err(CdpError::Timeout)` to each expired entry. On connection
close, `fail_all()` drains everything with `Err(CdpError::ChannelClosed)`.

### Event Fan-Out — `event_bus.rs`

`EventBus` distributes raw `Arc<EventFrame>` values to all registered
subscribers:

- **Global subscribers** receive every event regardless of session
- **Session subscribers** receive only events tagged with their `session_id`

Backpressure policy: if a channel is full the event is dropped for that
subscriber but the subscriber is retained. Disconnected receivers are pruned
on the next `publish` call.

### Target Registry — `target_tree.rs`

`TargetTree` is the single source of truth for all CDP targets. It replaces the old three-`Arc` triad (`target_cache`, `session_cells`, `destroy_watchers`) with one `Arc<RwLock<TargetTree>>` shared between `Handler` and `HandlerHandle`.

`TargetTree::apply_event` processes the five `Target.*` lifecycle events (`targetCreated`, `targetInfoChanged`, `targetDestroyed`, `attachedToTarget`, `detachedFromTarget`) and mutates the tree inline. The handler calls this **before** `EventBus::publish` so subscribers always observe an up-to-date tree when they react.

Each node (`TargetEntry`) stores the `TargetInfo` and an optional `SessionRef` (the current CDP session attached to that target). OOPIF iframes and workers are addressable via `TargetInfo.parent_id`.

### Frame State — `frame_tree.rs`

`FrameTree` tracks the live iframe hierarchy for a single `Page`. It ingests `Page.frameAttached`, `Page.frameNavigated`, `Page.frameDetached`, `Page.frameStartedLoading`, and `Page.frameStoppedLoading` events, plus `Runtime.executionContextCreated` / `Destroyed` to keep per-frame main-world and utility-world execution context IDs current.

`NavigationWaiter` uses the main-frame `FrameId` extracted from `FrameTree` to filter lifecycle events, avoiding false satisfaction from OOPIF subframe navigations. `Page::main_frame()` and `Page::frames()` are synchronous tree reads — zero CDP round-trips.

### Progress Token — `progress.rs`

`Progress` carries a wall-clock deadline and a shared `Arc<AtomicBool>` cancellation flag. Action retry loops call `Progress::check()` before each iteration; it returns `CdpError::Timeout` if either the deadline has passed or the cancel flag is set. `HandlerHandle` sets the shared flag on connection close so in-flight retry loops fail fast.

### Page and Element — `page/`, `element.rs`

`Page` represents a single browser tab. It holds a `HandlerHandle`, a
`TargetId`, and a `session_id` that scopes all commands to the correct CDP
session.

The `Page` type lives in `page/mod.rs` and its methods are split across
topical submodules via additional `impl Page` blocks: `navigation.rs`,
`content.rs`, `evaluation.rs`, `dom.rs`, `emulation.rs`, `cookies.rs`,
`capture.rs`, `input.rs`, `intercept.rs`, `events.rs`, `expose.rs`,
`device.rs`, `route.rs`, `websocket_route.rs`, `accessibility.rs`. Struct
fields use `pub(in crate::page)` visibility so submodules can access them
without exposing state to the rest of the crate.

Key design: `NavigationWaiter` is created *before* navigation is triggered to
avoid a race where the lifecycle event fires between the navigation command and
the subscribe call.

`Element` is a thin wrapper around a `NodeId` / `BackendNodeId` /
`RemoteObjectId` triple. DOM commands go through `Page::execute` scoped to the
page's session.

### Request Routing — `route.rs`

`RouteRegistry` holds an ordered list of `(glob_pattern, handler)` pairs behind `Arc<Mutex<…>>`. When a `Fetch.requestPaused` event arrives, the registry walks entries in registration order and calls the first matching handler. Unmatched requests are auto-continued.

`Route` wraps the paused request event and session; it exposes `fulfill(RouteResponse)`, `continue_req(opts)`, and `abort(reason)`. `Page::route(pattern, handler)` appends to the registry and starts the background `Fetch.requestPaused` subscription on first registration. `Page::unroute(pattern)` removes entries and stops the subscription when the registry is empty.

### Typed Event Streams — `listeners.rs`

`EventStream<T>` wraps a raw `mpsc::Receiver<Arc<EventFrame>>` and implements
`Stream<Item = T>`. On each `poll_next` it loops over incoming frames,
filtering by `T::method_id()` and deserializing `params` to `T`. Frames for
other methods are silently discarded.

---

## Data Flow

### Command flow (caller → browser → caller)

```
page.execute(NavigateParams { url: "…" })
  → HandlerHandle::execute()            serialize, allocate CallId, send HandlerMessage::Command
  → Handler::handle_caller()            conn.submit_command() → CallId registered in CommandDispatcher
  → Connection::poll_next() send path   drain pending_send queue, flush WebSocket
  ← browser JSON response               {"id":N,"result":{…}}
  → Handler::handle_incoming()          CommandDispatcher::resolve(N, Ok(value))
  → oneshot::Sender::send(value)
  ← caller await completes              deserialize to NavigateResponse
```

### Event flow (browser → subscriber)

```
  ← browser JSON event                  {"method":"Page.loadEventFired","params":{…}}
  → Handler::handle_incoming()          update TargetTree if Target.* event
  → EventBus::publish(Arc<EventFrame>)  fan-out to global + session subscribers
  → EventStream<LoadEventFiredEvent>    filter by method_id, deserialize, yield
  ← stream.next().await returns         LoadEventFiredEvent { … }
```

---

## Concurrency Model

- One `Handler` background task owns the connection and all mutable state
- `HandlerHandle` is `Clone + Send + Sync` — safe to share across tasks
- `TargetTree` is behind `Arc<RwLock<…>>` for lock-free reads on the hot
  path (target lookup never needs to go through the handler task)
- `CommandDispatcher` and `EventBus` live only inside the handler task — no
  locking needed for them

---

## Error Handling

`CdpError` (`error.rs`) is the single error type across the crate. Named variants cover every distinct failure mode: `Timeout`, `ChannelClosed`, `LockPoisoned`, `NotFound`, `JavascriptException`, `InvalidMessage`, `LaunchTimeout`, `LaunchExit`, and others. Large variants are `Box`ed (e.g. `WebSocket(Box<tungstenite::Error>)`, `JavascriptException(Box<ExceptionDetails>)`).

`CdpError::kind()` classifies every variant as `ErrorKind::Retry` (transient — re-resolve and retry), `ErrorKind::Fatal` (browser gone — abort), or `ErrorKind::Strict` (caller bug — surface immediately). Action loops use `CdpError::is_retryable()` to decide whether to retry.

---

## Feature Flags

| Flag | Effect |
|------|--------|
| `integration-tests` | Enables the browser-backed integration test suite |
| `_bench` | Exposes `CommandDispatcher`, `EventBus`, and `EventFrame` as `pub` for Criterion benchmarks in `crates/chromist/benches/`. Not part of the public API. |

## Benchmarks

See [`BENCHMARKS.md`](BENCHMARKS.md) for Criterion numbers covering `dispatch.rs` (register / resolve / sweep), `event_bus.rs` (global, session, and mixed fan-out), and `listeners.rs` (`EventStream` filter + deserialise). Reference run: Apple M1 Max, rustc 1.95.0.
