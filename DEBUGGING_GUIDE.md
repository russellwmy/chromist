# Debugging Guide — chromist

How to diagnose failures when Chrome is a separate process, potentially headless or in a container.

---

## Enable tracing output

Instrument output with:

```sh
# All chromist spans at debug level, everything else at warn
RUST_LOG=chromist=debug cargo run --example screenshot

# Trace-level (very verbose — every frame in/out)
RUST_LOG=chromist=trace cargo run --example screenshot

# Structured JSON output (requires tracing-subscriber with json feature)
RUST_LOG=chromist=debug cargo run --example screenshot 2>&1 | jq .
```

---

## Common failure modes

### `CdpError::LaunchNotFound`

No Chrome/Chromium executable was found on `PATH` or at any of the default lookup paths.

**Fix:** install Chrome/Chromium or point `BrowserConfig::builder().executable(path)` at the binary.

### `CdpError::LaunchPipeError { source }`

The `pipe()` syscall failed while setting up the OS-pipe transport between chromist and Chrome.

**Causes:**
- Process file-descriptor limit hit — raise `ulimit -n` or pass `--max-open-files` in Docker
- OS-level resource exhaustion

### `CdpError::LaunchSpawnError { source }`

`Command::spawn()` failed — the Chrome process could not be started.

**Causes:**
- Binary path is correct but not executable — run `chmod +x` on the binary
- Insufficient permissions in a restricted container — check `seccomp` / `AppArmor` profiles
- `source` wraps the underlying `std::io::Error`; check `source.kind()` for `PermissionDenied`, `NotFound`, etc.

### `CdpError::LaunchTimeout { stderr }`

Chrome did not print `DevTools listening on ws://` within the launch timeout.

**Causes:**
- No Chrome/Chromium binary found — run `which google-chrome chromium chromium-browser` to verify
- Binary exists but crashes immediately — run Chrome manually: `google-chrome --headless --remote-debugging-port=9222 about:blank` and check stderr
- Port already in use — the default port is 9222; change it via `BrowserConfig::builder().port(9223)`
- `--no-sandbox` required in container environments — add via `BrowserConfig::builder().args(["--no-sandbox"])`

### `CdpError::LaunchExit { exit_status, stderr }`

Chrome process exited before printing the WebSocket URL.

**Causes:**
- Missing system libraries in minimal Docker images — run `ldd $(which google-chrome)` to find missing `.so` files
- GPU/display errors in headless — add `--disable-gpu`, `--disable-dev-shm-usage`
- Insufficient `/dev/shm` — add `--shm-size=2gb` to Docker run, or pass `--disable-dev-shm-usage` to Chrome

### `CdpError::Timeout`

A CDP command did not receive a response within the request timeout (default: 30 s).

**Causes:**
- Page navigation hung waiting for a resource — use `WaitUntil::DomContentLoaded` instead of `NetworkIdle`
- Chrome tab crashed mid-operation
- The handler task exited before the response arrived — check for preceding `ChannelClosed` errors

To increase the per-command timeout, configure it at launch:
```rust
let config = BrowserConfig::builder()
    .request_timeout(Duration::from_secs(60))
    .build();
let browser = Browser::launch(config).await?;
```

### `CdpError::ChannelClosed`

The `HandlerHandle`'s channel to the background task is broken — the handler task exited.

**Causes:**
- The underlying WebSocket/pipe closed (`conn.next()` returned `None`)
- `Browser::kill()` was called — all subsequent commands will fail with this error
- The `HandlerHandle` was dropped while commands were still in flight

### `CdpError::InvalidMessage(raw, source)`

A JSON message arrived from Chrome that could not be deserialized into `Message<CdpJsonEventMessage>`.

**Diagnosis:** the `raw` field contains the unparsed string — log it:
```rust
if let Err(CdpError::InvalidMessage(raw, e)) = result {
    eprintln!("bad message: {raw}\nerror: {e}");
}
```

**Causes:**
- PDL bindings are stale relative to the installed Chrome version — run `cargo xtask codegen` after updating PDL files
- Chrome sent a CDP extension not present in the vendored PDL (rare)

### `CdpError::NotFound`

`HandlerHandle` could not find the target ID in the `TargetTree`.

**Cause:** `SetDiscoverTargets(true)` has not yet been acknowledged, or the target was destroyed before the lookup. The cache is populated by `Target.targetCreated` events — subscribe to those before querying.

### `CdpError::LockPoisoned`

The `TargetTree` or another internal `RwLock` was poisoned — another thread panicked while holding the write lock.

**Cause:** a panic while holding a write lock inside the handler task. Check for panics in the handler task logs. Once poisoned, all subsequent reads fail with this error.

---

## Attach to a running Chrome for manual inspection

```sh
# Start Chrome with a fixed debug port
google-chrome --headless --remote-debugging-port=9222 about:blank

# In a second terminal: confirm the endpoint is live
curl http://localhost:9222/json/version

# Connect chromist to the running instance (resolves the http endpoint to a ws URL)
let browser = Browser::connect_to_http("http://localhost:9222").await?;
```

This lets you keep Chrome alive between runs and inspect its state in the DevTools UI at `chrome://inspect`.

---

## Inspect raw CDP traffic

The protocol definition for every domain is at `crates/chromist-cdp/pdl/`. To see what a command sends and what the response looks like before implementing a wrapper:

1. Open Chrome DevTools (F12 in a headed window)
2. Go to **Settings → Experiments** → enable **Protocol Monitor**
3. Re-open DevTools → **More tools → Protocol monitor**

Alternatively, use the CDP reference at https://chromedevtools.github.io/devtools-protocol/ — it reflects the same PDL files vendored in `crates/chromist-cdp/pdl/`.

---

## Diagnose `Page` / navigation hangs

`NavigationWaiter` must be created **before** calling `goto` — creating it after means the lifecycle event may have already fired:

```rust
// Correct
let waiter = page.navigation_waiter(WaitUntil::NetworkIdle);
page.goto("https://example.com").await?;
waiter.wait().await?;

// Wrong — race condition; waiter may never fire
page.goto("https://example.com").await?;
let waiter = page.navigation_waiter(WaitUntil::NetworkIdle);
waiter.wait().await?; // may hang forever
```

If a waiter hangs, lower the wait condition: `WaitUntil::DomContentLoaded` fires earlier than `NetworkIdle`.

---

## Diagnose event stream stalls

`EventStream<T>` silently discards frames whose method does not match `T::method_id()`. If your stream produces no items:

1. Verify the type — print `T::method_id()` and compare against what Chrome actually sends via the Protocol Monitor.
2. Verify the session scope — a session-scoped subscriber (`page.event_listener::<T>()`) only sees events tagged with the page's `session_id`. Top-level browser events have no session ID and require a global subscriber (`handle.event_listener::<T>(None)`).
3. Check channel capacity — the event channel holds 512 frames (`EVENT_CHANNEL_CAP`). If your consumer is slow and Chrome is sending many events, early frames are dropped silently. Consume in a tight loop or increase the cap.
