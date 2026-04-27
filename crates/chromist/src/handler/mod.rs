//! Background I/O multiplexer that owns the CDP connection.
//!
//! The `Handler` task runs a `futures::select!` loop that drives three
//! sources concurrently: inbound messages from the browser (responses and
//! events), outbound command requests from callers, and a 500 ms timeout
//! sweep tick. It delegates request/response correlation to
//! [`CommandDispatcher`](crate::dispatch::CommandDispatcher) and event fan-out
//! to [`EventBus`](crate::event_bus::EventBus).
//!
//! Callers never hold a direct reference to the handler task. Instead they
//! use the cheaply cloneable [`HandlerHandle`](crate::HandlerHandle), which sends [`HandlerMessage`]
//! variants over an unbounded mpsc channel. The handle also shares the
//! handler's `target_cache` via an `Arc<RwLock<…>>` so target lookups are
//! lock-free reads rather than message round-trips.
//!
//! The cache is updated *before* the corresponding `Target.targetCreated` /
//! `targetDestroyed` / `targetInfoChanged` event is published, so subscribers
//! never observe a stale view.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chromist_types::{CdpJsonEventMessage, Message};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use serde_json::Value;

use crate::cdp::browser_protocol::target as cdp_target;
use crate::cmd::{EventFrame, HandlerMessage};
use crate::conn::AnyConnection;
use crate::dispatch::CommandDispatcher;
use crate::error::CdpError;
use crate::event_bus::EventBus;
use crate::layout::Viewport;
use crate::target_tree::TargetTree;

pub(crate) mod context;
pub(crate) mod session;
pub use context::BrowserContext;
pub(crate) use context::{ContextBindingHandler, ContextExposeHandler};
pub(crate) use session::SessionRef;

/// A proxy for registering event subscribers at the handler level.
///
/// Returned by [`HandlerHandle::event_listeners_mut`]. Provides the same
/// subscription surface as [`HandlerHandle`](crate::HandlerHandle) without requiring a mutable
/// reference to the handle itself.
#[derive(Debug)]
pub struct EventListeners(HandlerHandle);

impl EventListeners {
    /// Subscribe to raw events for all methods, optionally scoped to a session.
    pub fn subscribe(&self, session_id: Option<Arc<str>>) -> mpsc::Receiver<Arc<EventFrame>> {
        self.0.subscribe(session_id)
    }

    /// Subscribe to typed events for a specific CDP event type, optionally
    /// scoped to a session.
    pub fn event_listener<T>(
        &self,
        session_id: Option<Arc<str>>,
    ) -> crate::listeners::EventStream<T>
    where
        T: chromist_types::EventMessage + chromist_types::MethodType + Send + 'static,
    {
        self.0.event_listener(session_id)
    }
}

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SWEEP_INTERVAL: Duration = Duration::from_millis(500);

/// Thin orchestrator that delegates to [`CommandDispatcher`] (request-response
/// correlation + timeouts) and [`EventBus`] (event fan-out).
pub(crate) struct Handler {
    conn: AnyConnection,
    from_caller: mpsc::UnboundedReceiver<HandlerMessage>,
    dispatch: CommandDispatcher,
    bus: EventBus,
    /// Event-driven registry of all known targets, their attached sessions, and
    /// their destroy-watcher flags. Updated before events are published to the
    /// bus, so subscribers never observe a stale tree.
    tree: Arc<RwLock<TargetTree>>,
    /// Shared flag set when the connection closes.
    cancel_token: Arc<AtomicBool>,
}

/// Cheaply cloneable handle used by callers to send commands and subscribe to
/// events without needing direct access to the `Handler` task.
#[derive(Clone, Debug)]
pub struct HandlerHandle {
    tx: mpsc::UnboundedSender<HandlerMessage>,
    /// Shared reference to the handler's target tree — reads are lock-free on
    /// the happy path and require no message round-trip to the handler task.
    pub(crate) tree: Arc<RwLock<TargetTree>>,
    /// Shared cancellation flag — set when the connection closes. Exposed via
    /// [`progress`](Self::progress) so action loops can fail fast after the
    /// browser exits without waiting out their full deadline.
    pub(crate) cancel_token: Arc<AtomicBool>,
}

impl HandlerHandle {
    /// Register a shared `AtomicBool` that will be set to `true` when the
    /// handler observes a `Target.targetDestroyed` event for `target_id`.
    pub(crate) fn register_destroy_watcher(
        &self,
        target_id: &cdp_target::TargetId,
        flag: &Arc<AtomicBool>,
    ) {
        if let Ok(mut tree) = self.tree.write() {
            tree.register_destroy_watcher(target_id, flag);
        }
    }

    /// Return the live `SessionRef` for a target, if it is currently attached.
    pub fn session_cell(&self, target_id: &cdp_target::TargetId) -> Option<SessionRef> {
        self.tree.read().ok()?.session_cell(target_id)
    }

    /// Return the existing `SessionRef` for `target_id`, or create one
    /// initialised to `initial`. If the cell already exists its stored ID is
    /// overwritten with `initial` (renderer-swap behaviour).
    pub(crate) fn ensure_session_cell(
        &self,
        target_id: &cdp_target::TargetId,
        initial: Arc<str>,
    ) -> SessionRef {
        if let Ok(mut tree) = self.tree.write() {
            return tree.ensure_session(target_id, initial.clone());
        }
        // Lock poisoned — return a detached cell so the caller still functions.
        SessionRef::new(initial)
    }
}

/// Configuration for the CDP handler.
#[derive(Debug, Clone)]
pub struct HandlerConfig {
    /// Pass `--ignore-certificate-errors` to the browser.
    pub ignore_https_errors: bool,
    /// Drop CDP messages that fail to deserialize at `trace!` level
    /// instead of surfacing them as errors.
    pub ignore_invalid_messages: bool,
    /// Initial viewport applied to newly attached targets.
    pub viewport: Option<Viewport>,
    /// Default per-CDP-command timeout used by the dispatcher.
    pub request_timeout: Duration,
    /// Enable `Fetch.enable` request interception by default for new
    /// pages spawned through this handler.
    pub request_intercept: bool,
    /// Cache enabled.
    pub cache_enabled: bool,
    /// Pre-registered browser context IDs.
    ///
    /// When connecting to an already-running browser, supply the known
    /// non-default `BrowserContextId`s here so the handler can route events
    /// correctly without first enumerating them. Empty means "default context
    /// only" — additional contexts discovered at runtime are still tracked.
    pub context_ids: Vec<crate::cdp::browser_protocol::browser::BrowserContextId>,
}

impl Default for HandlerConfig {
    fn default() -> Self {
        HandlerConfig {
            ignore_https_errors: true,
            ignore_invalid_messages: true,
            viewport: Some(Viewport::new(800, 600)),
            request_timeout: Duration::from_secs(30),
            request_intercept: false,
            cache_enabled: true,
            context_ids: Vec::new(),
        }
    }
}

impl HandlerHandle {
    #[tracing::instrument(skip_all, fields(method = %cmd.identifier()))]
    /// Send a typed CDP command and await its response.
    ///
    /// Optionally scoped to a target session via `session_id`. Returns the
    /// command's typed `Response` on success or a [`CdpError`](crate::CdpError)
    /// on transport failure / CDP-level error / timeout.
    pub async fn execute<C>(
        &self,
        cmd: C,
        session_id: Option<Arc<str>>,
    ) -> crate::Result<C::Response>
    where
        C: chromist_types::Command,
    {
        let method = cmd.identifier();
        let params = serde_json::to_value(&cmd)?;
        let (tx, rx) = oneshot::channel();
        self.tx
            .unbounded_send(HandlerMessage::Command { method, params, session_id, tx })
            .map_err(|_| CdpError::ChannelClosed)?;
        let value = rx.await.map_err(|_| CdpError::ChannelClosed)??;
        Ok(serde_json::from_value(value)?)
    }

    /// Subscribe to the raw event stream, optionally scoped to a session.
    ///
    /// The returned receiver is bounded; events are dropped (not the subscriber)
    /// when the channel is full.  The [`EventBus`] inside the handler owns the
    /// fan-out logic and prunes disconnected receivers automatically.
    pub fn subscribe(&self, session_id: Option<Arc<str>>) -> mpsc::Receiver<Arc<EventFrame>> {
        let (tx, rx) = mpsc::channel(crate::cmd::EVENT_CHANNEL_CAP);
        let _ = self.tx.unbounded_send(HandlerMessage::Subscribe { session_id, tx });
        rx
    }

    /// Send a shutdown signal to the handler task. Subsequent `execute`
    /// calls will fail with [`CdpError::ChannelClosed`](crate::CdpError::ChannelClosed).
    pub fn shutdown(&self) {
        let _ = self.tx.unbounded_send(HandlerMessage::Shutdown);
    }

    /// Create a [`Progress`](crate::progress::Progress) that shares this
    /// handler's cancel flag.  When the browser connection closes the flag is
    /// set and every live `Progress` created this way will return
    /// `Err(Timeout)` from [`check`](crate::progress::Progress::check).
    pub fn progress(&self, timeout: std::time::Duration) -> crate::progress::Progress {
        crate::progress::Progress::with_shared_cancel(timeout, Arc::clone(&self.cancel_token))
    }

    /// Look up a single target by ID from the local tree — no CDP round-trip.
    ///
    /// Returns `CdpError::NotFound` if the target ID is not in the tree yet.
    /// The tree is populated by `Target.targetCreated` events emitted after
    /// `SetDiscoverTargets(true)`, so all targets that exist when the browser was
    /// launched or connected will be present after the first event loop tick.
    pub fn target(&self, id: cdp_target::TargetId) -> crate::Result<cdp_target::TargetInfo> {
        self.tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .get(&id)
            .map(|e| e.info.clone())
            .ok_or(CdpError::NotFound)
    }

    /// Return all currently known targets from the local tree — no CDP round-trip.
    pub fn targets(&self) -> crate::Result<Vec<cdp_target::TargetInfo>> {
        Ok(self
            .tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .all()
            .map(|e| e.info.clone())
            .collect())
    }

    /// Return all children of `parent_id` — targets whose `TargetInfo.parent_id`
    /// matches (iframe targets, workers, OOPIFs).
    pub fn children_of(
        &self,
        parent_id: &cdp_target::TargetId,
    ) -> crate::Result<Vec<cdp_target::TargetInfo>> {
        Ok(self
            .tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .children_of(parent_id)
            .map(|e| e.info.clone())
            .collect())
    }

    /// Return all non-default browser context IDs via CDP.
    pub async fn browser_context_ids(
        &self,
    ) -> crate::Result<Vec<crate::cdp::browser_protocol::browser::BrowserContextId>> {
        use crate::cdp::browser_protocol::target as cdp_target;
        let resp = self.execute(cdp_target::GetBrowserContextsParams::default(), None).await?;
        Ok(resp.browser_context_ids)
    }

    /// Returns a handle to the default (non-incognito) browser context.
    ///
    /// The default context has `id == None`. Use [`browser_context_ids`] to
    /// enumerate all non-default contexts.
    pub fn default_browser_context(&self) -> BrowserContext {
        BrowserContext::new_with_id(self.clone(), None)
    }

    /// Return all browser contexts: the default context followed by all
    /// non-default (incognito) contexts known to the browser.
    pub async fn browser_contexts(&self) -> crate::Result<Vec<BrowserContext>> {
        let mut contexts = vec![BrowserContext::new_with_id(self.clone(), None)];
        for id in self.browser_context_ids().await? {
            contexts.push(BrowserContext::new_with_id(self.clone(), Some(id)));
        }
        Ok(contexts)
    }

    /// Returns an [`EventListeners`] proxy for registering event subscribers.
    ///
    /// Under the hood it delegates to the same channel-based subscription
    /// mechanism as [`subscribe`] and [`event_listener`].
    pub fn event_listeners_mut(&self) -> EventListeners {
        EventListeners(self.clone())
    }
}

impl Handler {
    pub(crate) fn new(conn: AnyConnection) -> (Self, HandlerHandle) {
        let (tx, rx) = mpsc::unbounded();
        let tree = Arc::new(RwLock::new(TargetTree::new()));
        let cancel_token = Arc::new(AtomicBool::new(false));
        let handler = Handler {
            conn,
            from_caller: rx,
            dispatch: CommandDispatcher::new(DEFAULT_REQUEST_TIMEOUT),
            bus: EventBus::new(),
            tree: Arc::clone(&tree),
            cancel_token: Arc::clone(&cancel_token),
        };
        (handler, HandlerHandle { tx, tree, cancel_token })
    }

    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.dispatch = CommandDispatcher::new(timeout);
        self
    }

    pub(crate) fn spawn(conn: AnyConnection) -> HandlerHandle {
        let (handler, handle) = Self::new(conn);
        crate::runtime::spawn(handler.run());
        handle
    }

    pub(crate) async fn run(mut self) {
        let mut tick = Box::pin(crate::runtime::sleep(SWEEP_INTERVAL).fuse());
        loop {
            futures::select! {
                msg = self.conn.next().fuse() => match msg {
                    Some(Ok(m)) => {
                        tracing::trace!(message_type = ?std::mem::discriminant(&m), "inbound browser message");
                        self.handle_incoming(m);
                    }
                    Some(Err(e)) => {
                        tracing::trace!(error = %e, "connection error; shutting down handler");
                        self.fail_all_closed();
                        break;
                    }
                    None => {
                        tracing::trace!("connection closed; shutting down handler");
                        self.fail_all_closed();
                        break;
                    }
                },
                cmd = self.from_caller.next().fuse() => match cmd {
                    Some(HandlerMessage::Shutdown) | None => break,
                    Some(other) => {
                        tracing::trace!("inbound caller message");
                        self.handle_caller(other);
                    }
                },
                _ = tick => {
                    self.dispatch.sweep_timeouts();
                    tick = Box::pin(crate::runtime::sleep(SWEEP_INTERVAL).fuse());
                }
            }
        }
    }

    fn handle_incoming(&mut self, msg: Message<CdpJsonEventMessage>) {
        match msg {
            Message::Response(resp) => {
                let result = if let Some(err) = resp.error {
                    Err(CdpError::Cdp { code: err.code, message: err.message })
                } else {
                    Ok(resp.result.unwrap_or(Value::Null))
                };
                self.dispatch.resolve(resp.id, result);
            }
            Message::Event(ev) => {
                // Update the target cache before publishing so that any
                // subscriber that reacts to this event sees a consistent cache.
                self.apply_to_tree(&ev.method, &ev.params);
                tracing::trace!(method = %ev.method, session_id = ?ev.session_id, "CDP event");
                let frame = Arc::new(EventFrame {
                    method: ev.method,
                    params: ev.params,
                    session_id: ev.session_id,
                });
                self.bus.publish(frame);
            }
            _ => {}
        }
    }

    fn apply_to_tree(&self, method: &str, params: &Value) {
        match self.tree.write() {
            Ok(mut tree) => tree.apply_event(method, params),
            Err(_) => tracing::warn!(
                method,
                "target tree lock poisoned; lifecycle event not applied — \
                 subscribers may observe stale target state"
            ),
        }
    }

    fn handle_caller(&mut self, msg: HandlerMessage) {
        match msg {
            HandlerMessage::Command { method, params, session_id, tx } => {
                let id = self.conn.submit_command(method, session_id, params);
                self.dispatch.register(id, tx);
            }
            HandlerMessage::Subscribe { session_id, tx } => {
                self.bus.subscribe_with_sender(session_id, tx);
            }
            HandlerMessage::Shutdown => {}
        }
    }

    fn fail_all_closed(&mut self) {
        self.cancel_token.store(true, Ordering::Release);
        self.dispatch.fail_all();
        self.bus.close_all();
    }
}

#[cfg(test)]
mod tests {
    //! Handler-level integration tests using an in-process mock transport.
    //!
    //! These tests assert the load-bearing invariants documented in the
    //! module: tree-before-publish ordering, destroy-watcher fan-out,
    //! `fail_all_closed` on disconnect, and session-cell updates on
    //! `Target.attachedToTarget` (including the cross-process renderer-swap
    //! case where the same `targetId` re-attaches under a fresh session id).

    use super::*;
    use crate::conn::mock::{MockConnection, MockHandle};
    use chromist_types::{CdpJsonEventMessage, Message, Response};
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    fn event(method: &str, params: Value) -> Message<CdpJsonEventMessage> {
        Message::Event(CdpJsonEventMessage { method: method.to_string(), params, session_id: None })
    }

    /// Spin briefly until `pred` returns `Some`. Lets event-driven state
    /// (tree, session cells, destroy watchers) settle before assertion.
    async fn poll_for<T>(mut pred: impl FnMut() -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(v) = pred() {
                return v;
            }
            if std::time::Instant::now() >= deadline {
                panic!("timed out waiting for handler state");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn tree_updated_before_event_published() {
        // Subscribers reading the target tree from inside an event handler
        // must observe the post-update state, never the pre-update state.
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let mut sub = handle.subscribe(None);
        mock.inbound
            .unbounded_send(Ok(event(
                "Target.targetCreated",
                json!({
                    "targetInfo": {
                        "targetId": "T-1",
                        "type": "page",
                        "title": "",
                        "url": "about:blank",
                        "attached": false,
                        "canAccessOpener": false,
                    }
                }),
            )))
            .unwrap();

        let frame = sub.next().await.expect("event delivered");
        assert_eq!(frame.method, "Target.targetCreated");
        // At the moment the event is observable, the cache must already
        // contain the target.
        let target = handle
            .target(crate::cdp::browser_protocol::target::TargetId::from("T-1".to_string()))
            .expect("tree populated before publish");
        assert_eq!(target.target_id.inner().as_str(), "T-1");

        handle.shutdown();
    }

    #[tokio::test]
    async fn session_cell_populated_on_attached_to_target() {
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        mock.inbound
            .unbounded_send(Ok(event(
                "Target.attachedToTarget",
                json!({
                    "sessionId": "S-1",
                    "targetInfo": {
                        "targetId": "T-1",
                        "type": "page",
                        "title": "",
                        "url": "about:blank",
                        "attached": true,
                        "canAccessOpener": false,
                    },
                    "waitingForDebugger": false,
                }),
            )))
            .unwrap();

        let cell = poll_for(|| {
            handle.session_cell(&crate::cdp::browser_protocol::target::TargetId::from(
                "T-1".to_string(),
            ))
        })
        .await;
        assert_eq!(&*cell.current(), "S-1");

        handle.shutdown();
    }

    #[tokio::test]
    async fn session_cell_updates_on_renderer_swap() {
        // The exact failure mode the pipe-only `-32001` bug exercised:
        // same target_id, two attached events, second one carries a fresh
        // sessionId. Holders of the cell must transparently observe S-2.
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let target_info = json!({
            "targetId": "T-1",
            "type": "page",
            "title": "",
            "url": "about:blank",
            "attached": true,
            "canAccessOpener": false,
        });

        mock.inbound
            .unbounded_send(Ok(event(
                "Target.attachedToTarget",
                json!({ "sessionId": "S-1", "targetInfo": target_info, "waitingForDebugger": false }),
            )))
            .unwrap();

        let cell = poll_for(|| {
            handle.session_cell(&crate::cdp::browser_protocol::target::TargetId::from(
                "T-1".to_string(),
            ))
        })
        .await;
        assert_eq!(&*cell.current(), "S-1");

        mock.inbound
            .unbounded_send(Ok(event(
                "Target.attachedToTarget",
                json!({ "sessionId": "S-2", "targetInfo": target_info, "waitingForDebugger": false }),
            )))
            .unwrap();

        // Same Arc<RwLock<…>> backing — observe the swap without re-fetching
        // the cell from the registry.
        poll_for(|| if &*cell.current() == "S-2" { Some(()) } else { None }).await;

        handle.shutdown();
    }

    #[tokio::test]
    async fn destroy_watcher_fires_on_target_destroyed() {
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let flag = Arc::new(AtomicBool::new(false));
        let target_id = crate::cdp::browser_protocol::target::TargetId::from("T-1".to_string());
        handle.register_destroy_watcher(&target_id, &flag);

        mock.inbound
            .unbounded_send(Ok(event("Target.targetDestroyed", json!({ "targetId": "T-1" }))))
            .unwrap();

        poll_for(|| if flag.load(Ordering::Acquire) { Some(()) } else { None }).await;

        handle.shutdown();
    }

    #[tokio::test]
    async fn disconnect_fails_pending_callers() {
        // Drop the inbound sender → handler observes None on the read arm
        // → `fail_all_closed` resolves every pending oneshot with
        // `ChannelClosed`, never hangs.
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        // Issue a command; the handler will register it but no response
        // will ever arrive.
        let pending = tokio::spawn({
            let h = handle.clone();
            async move {
                h.execute(
                    crate::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true),
                    None,
                )
                .await
            }
        });

        // Wait until the handler has actually sent the command so we know
        // the dispatcher has registered the oneshot.
        let MockHandle { mut outbound, inbound } = mock;
        let _cmd = tokio::time::timeout(Duration::from_secs(2), outbound.next())
            .await
            .expect("handler dispatched command")
            .expect("command stream alive");

        drop(inbound); // simulate transport disconnect

        let res = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .expect("pending caller resolved")
            .expect("task did not panic");
        assert!(matches!(res, Err(CdpError::ChannelClosed)), "got {:?}", res);
    }

    #[tokio::test]
    async fn response_correlation_round_trips() {
        // Sanity check on the dispatcher integration: a typed command's
        // oneshot resolves with the response payload.
        let (conn, mut mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let task = tokio::spawn({
            let h = handle.clone();
            async move {
                h.execute(
                    crate::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true),
                    None,
                )
                .await
            }
        });

        let cmd = tokio::time::timeout(Duration::from_secs(2), mock.outbound.next())
            .await
            .expect("dispatched")
            .expect("alive");
        let id = cmd.id;

        mock.inbound
            .unbounded_send(Ok(Message::Response(Box::new(Response {
                id,
                result: Some(json!({})),
                error: None,
            }))))
            .unwrap();

        let res = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("resolved")
            .expect("no panic");
        assert!(res.is_ok(), "expected Ok, got {:?}", res);
        handle.shutdown();
    }
}
