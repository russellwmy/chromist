#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
#![warn(unreachable_pub)]
#![warn(noop_method_call)]
#![warn(trivial_numeric_casts)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]

//! A Rust client for the Chrome DevTools Protocol.
//!
//! # Quickstart
//!
//! ```no_run
//! use chromist::{Browser, BrowserConfig};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let browser = Browser::launch_default().await?;
//! let page = browser.new_page("https://example.com").await?;
//! let png = page.screenshot().await?;
//! std::fs::write("out.png", &png)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Evaluating JavaScript
//!
//! ```no_run
//! # use chromist::{Browser, BrowserConfig};
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! # let browser = Browser::launch(BrowserConfig::builder().build()).await?;
//! # let page = browser.new_page("about:blank").await?;
//! let result = page.evaluate("document.title").await?;
//! let title: String = result.into_value()?;
//! println!("title: {title}");
//! # Ok(())
//! # }
//! ```
//!
//! # Waiting for navigation
//!
//! ```no_run
//! # use chromist::{Browser, BrowserConfig, WaitUntil};
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! # let browser = Browser::launch(BrowserConfig::builder().build()).await?;
//! # let page = browser.new_page("about:blank").await?;
//! let waiter = page.navigation_waiter(WaitUntil::NetworkIdle);
//! page.goto("https://example.com").await?;
//! waiter.wait().await?;
//! # Ok(())
//! # }
//! ```

pub(crate) mod accessibility;
pub(crate) mod api_request;
pub(crate) mod auth;
pub(crate) mod browser;
pub(crate) mod cdp_session;
pub(crate) mod clock;
#[cfg(feature = "_bench")]
#[doc(hidden)]
pub(crate) mod cmd;
#[cfg(not(feature = "_bench"))]
pub(crate) mod cmd;
pub(crate) mod conn;
pub(crate) mod context;
pub(crate) mod context_options;
pub(crate) mod coverage;
pub(crate) mod detection;
pub(crate) mod device;
pub(crate) mod dialog;
#[cfg(feature = "_bench")]
#[doc(hidden)]
pub(crate) mod dispatch;
#[cfg(not(feature = "_bench"))]
pub(crate) mod dispatch;
pub(crate) mod element;
pub(crate) mod error;
pub(crate) mod evaluate;
#[cfg(feature = "_bench")]
#[doc(hidden)]
pub(crate) mod event_bus;
#[cfg(not(feature = "_bench"))]
pub(crate) mod event_bus;
pub(crate) mod events;
pub(crate) mod file_chooser;
pub(crate) mod frame;
pub(crate) mod frame_locator;
pub(crate) mod frame_tree;
pub(crate) mod handler;
pub(crate) mod har;
pub(crate) mod har_recorder;
pub(crate) mod js;
pub(crate) mod js_handle;
pub(crate) mod keys;
pub(crate) mod layout;
pub(crate) mod lifecycle;
pub(crate) mod listeners;
pub(crate) mod locator;
pub(crate) mod network;
pub(crate) mod page;
#[cfg(unix)]
pub(crate) mod pipe_conn;
pub(crate) mod progress;
pub(crate) mod route;
pub(crate) mod runtime;
pub(crate) mod screenshot;
pub(crate) mod storage;
pub(crate) mod target_tree;
pub(crate) mod tracing;
/// Utility helpers — currently base64 encode / decode for CDP payloads.
pub mod utils;
pub(crate) mod websocket;

/// Unstable surface exposed only under the internal `_bench` feature for
/// criterion benches in `benches/`. Not part of the public API.
#[cfg(feature = "_bench")]
#[doc(hidden)]
pub mod __bench {
    pub use crate::cmd::{EventFrame, EVENT_CHANNEL_CAP};
    pub use crate::dispatch::CommandDispatcher;
    pub use crate::event_bus::EventBus;
}

pub use accessibility::AXNode;
pub use api_request::{APIRequestContext, APIResponse, FetchOptions};
pub use auth::Credentials;
pub use browser::browser_connection;
pub use browser::config::HeadlessMode;
pub use browser::{Browser, BrowserConfig, BrowserConfigBuilder};
pub use cdp::browser_protocol::browser::PermissionType;
pub use cdp_session::CdpSession;
pub use chromist_cdp::cdp;
pub use chromist_types as types;
pub use chromist_types::{Binary, Command, Method, MethodType};
pub use clock::Clock;
pub use cmd::EventFrame;
#[doc(hidden)]
pub use conn::{AnyConnection, Connection};
pub use context::PageWaiter;
pub use context_options::{
    BrowserContextOptions, BrowserContextOptionsBuilder, HttpCredentials, ServiceWorkersPolicy,
};
pub use coverage::{CssCoverage, JsCoverage};
pub use detection::{DetectionOptions, StealthOptions};
pub use device::DeviceDescriptor;
pub use dialog::{DialogHandler, DialogType};
pub use element::{AttributeStream, Element};
pub use error::{CdpError, ErrorKind};
pub use evaluate::EvaluationResult;
pub use events::{ConsoleLocation, ConsoleMessage, Download, Worker};
pub use file_chooser::{FileChooser, FileChooserMode};
pub use frame::Frame;
pub use frame_locator::FrameLocator;
pub use handler::BrowserContext;
#[doc(hidden)]
pub use handler::{EventListeners, HandlerConfig, HandlerHandle};
pub use har::{
    Har, HarContent, HarCreator, HarEntry, HarHeader, HarLog, HarNotFound, HarOptions, HarPostData,
    HarRequest, HarResponse, HarTimings,
};
pub use har_recorder::{HarRecorder, HarRecordingOptions};
pub use js_handle::JsHandle;
pub use keys::{key_definition, KeyDefinition, USKEYBOARD_LAYOUT};
pub use layout::{BoundingBox, BoxModel, ElementQuad, Point, Viewport};
pub use lifecycle::{GotoOptions, NavigationWaiter, ReloadOptions, WaitUntil};
pub use listeners::{CustomEvent, EventStream};
pub use locator::{Expect, Locator};
pub use network::{ArcHttpRequest, BrowserConnection, HttpRequest, NetworkManager};
pub use page::cookies::Cookies;
pub use page::dom::Dom;
pub use page::emulation::Emulation;
pub use page::events::Events;
pub use page::input::{ClickOptions, Keyboard, Mouse, MouseButton, Touchscreen};
pub use page::websocket_route::{WebSocketRoute, WsRouteHandler};
pub use page::{MediaType, Page, ScriptSource};
// `InitScriptId` and `PerfMetrics` are defined directly in this file (below).
pub use progress::Progress;
pub use route::{AbortReason, Route, RouteOverride, RouteRegistry, RouteResponse};
#[doc(hidden)]
pub use runtime::AbortOnDrop;
pub use screenshot::{ScreenshotFormat, ScreenshotParams, ScreenshotParamsBuilder};
pub use storage::{Cookie, OriginStorage, StorageState};
pub use tracing::TracingSession;
pub use websocket::{WebSocket, WebSocketEvent, WebSocketEventKind};

/// Crate-wide `Result` alias for fallible chromist operations.
///
/// Mirrors the `std::io::Result` / `tokio::io::Result` convention.
pub type Result<T> = std::result::Result<T, CdpError>;

/// Common imports for everyday usage.
///
/// Bring the essentials into scope with a single glob:
///
/// ```no_run
/// use chromist::prelude::*;
///
/// # async fn run() -> Result<()> {
/// let browser = Browser::launch(BrowserConfig::builder().build()).await?;
/// let page = browser.new_page("https://example.com").await?;
/// page.goto("https://example.com").await?;
/// # Ok(())
/// # }
/// ```
pub mod prelude {
    pub use crate::browser::{Browser, BrowserConfig, BrowserConfigBuilder};
    pub use crate::context::PageWaiter;
    pub use crate::detection::StealthOptions;
    pub use crate::element::Element;
    pub use crate::error::CdpError;
    pub use crate::events::{ConsoleMessage, Download};
    pub use crate::frame::Frame;
    pub use crate::lifecycle::{GotoOptions, NavigationWaiter, ReloadOptions, WaitUntil};
    pub use crate::listeners::EventStream;
    pub use crate::locator::{Expect, Locator};
    pub use crate::page::{MediaType, Page, ScriptSource};
    pub use crate::screenshot::ScreenshotFormat;
    pub use crate::storage::Cookie;
    pub use crate::Result;
}

/// Outcome of a navigation initiated via [`Page::goto`], [`Page::goto_with`],
/// or [`Frame::goto`].
///
/// `Page::goto` returns `Ok(Navigation)` only after the navigation has
/// committed without an `error_text` from CDP. The wrapper exposes the
/// committed frame, the loader id (absent for same-document navigations such
/// as `pushState`), and a `is_download` flag indicating whether Chromium
/// converted the navigation into a download. For unwrapped CDP access, use
/// [`Navigation::into_inner`].
#[derive(Debug, Clone)]
pub struct Navigation {
    inner: cdp::browser_protocol::page::NavigateResponse,
}

impl Navigation {
    /// Frame that committed (or attempted) the navigation.
    pub fn frame_id(&self) -> &cdp::browser_protocol::page::FrameId {
        &self.inner.frame_id
    }

    /// Loader id for the new document, or `None` for same-document
    /// navigations (`pushState` / hash change).
    pub fn loader_id(&self) -> Option<&cdp::browser_protocol::network::LoaderId> {
        self.inner.loader_id.as_ref()
    }

    /// `true` when Chromium converted the navigation into a download
    /// instead of rendering a document.
    pub fn is_download(&self) -> bool {
        self.inner.is_download.unwrap_or(false)
    }

    /// Borrow the underlying CDP `NavigateResponse`.
    pub fn as_inner(&self) -> &cdp::browser_protocol::page::NavigateResponse {
        &self.inner
    }

    /// Consume the wrapper and return the underlying CDP `NavigateResponse`.
    pub fn into_inner(self) -> cdp::browser_protocol::page::NavigateResponse {
        self.inner
    }
}

impl From<cdp::browser_protocol::page::NavigateResponse> for Navigation {
    fn from(inner: cdp::browser_protocol::page::NavigateResponse) -> Self {
        Navigation { inner }
    }
}

/// A snapshot of a DOM node returned by [`Page::dom`](crate::Page::dom)`.document()`,
/// [`Dom::describe_node`](crate::Dom::describe_node), or [`Element::description`].
///
/// Wraps the underlying CDP `DOM.Node` payload — a polymorphic record
/// covering Element, Document, Attr, Shadow DOM, and other node kinds. The
/// methods on this type expose the common-case accessors; for fields that
/// are kind-specific (document URL, public/system ID, pseudo type, etc.)
/// drop down to the inner record via [`DomNode::into_inner`].
#[derive(Debug, Clone)]
pub struct DomNode {
    inner: cdp::browser_protocol::dom::Node,
}

impl DomNode {
    /// Node identifier — passes back into other DOM commands.
    pub fn node_id(&self) -> cdp::browser_protocol::dom::NodeId {
        self.inner.node_id
    }

    /// Backend node identifier — stable across navigations within a session.
    pub fn backend_node_id(&self) -> cdp::browser_protocol::dom::BackendNodeId {
        self.inner.backend_node_id
    }

    /// Parent node id, if this node has a parent in the snapshot.
    pub fn parent_id(&self) -> Option<cdp::browser_protocol::dom::NodeId> {
        self.inner.parent_id
    }

    /// DOM `nodeType` (1 = Element, 3 = Text, 9 = Document, …).
    pub fn node_type(&self) -> i64 {
        self.inner.node_type
    }

    /// DOM `nodeName`. For elements this is the upper-case tag name.
    pub fn node_name(&self) -> &str {
        &self.inner.node_name
    }

    /// DOM `localName`. For elements this is the lower-case tag name.
    pub fn local_name(&self) -> &str {
        &self.inner.local_name
    }

    /// DOM `nodeValue` — text content for text nodes, the attribute value
    /// for attr nodes, empty string for elements.
    pub fn node_value(&self) -> &str {
        &self.inner.node_value
    }

    /// Number of children, when known. Always present on container kinds;
    /// `None` on leaf node types.
    pub fn child_count(&self) -> Option<i64> {
        self.inner.child_node_count
    }

    /// Children when fetched with `pierce` / `depth > 1`.
    pub fn children(&self) -> Option<impl Iterator<Item = DomNode> + '_> {
        self.inner.children.as_ref().map(|cs| cs.iter().cloned().map(DomNode::from))
    }

    /// Element attributes as `(name, value)` pairs. CDP packs them as a flat
    /// `[name1, value1, name2, value2, …]` array; this iterator yields the
    /// pairs. Returns an empty iterator for non-element nodes.
    pub fn attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        let attrs = self.inner.attributes.as_deref().unwrap_or(&[]);
        attrs.chunks_exact(2).map(|c| (c[0].as_str(), c[1].as_str()))
    }

    /// `true` when `node_type == 1` (Element).
    pub fn is_element(&self) -> bool {
        self.inner.node_type == 1
    }

    /// `true` when `node_type == 3` (Text).
    pub fn is_text(&self) -> bool {
        self.inner.node_type == 3
    }

    /// `true` when `node_type == 9` (Document).
    pub fn is_document(&self) -> bool {
        self.inner.node_type == 9
    }

    /// Borrow the underlying CDP record for kind-specific fields not
    /// surfaced by the chromist accessors.
    pub fn as_inner(&self) -> &cdp::browser_protocol::dom::Node {
        &self.inner
    }

    /// Consume the wrapper and return the underlying CDP record.
    pub fn into_inner(self) -> cdp::browser_protocol::dom::Node {
        self.inner
    }
}

impl From<cdp::browser_protocol::dom::Node> for DomNode {
    fn from(inner: cdp::browser_protocol::dom::Node) -> Self {
        DomNode { inner }
    }
}

/// Layout metrics returned by [`Page::layout_metrics`].
///
/// Wraps the CDP `Page.getLayoutMetrics` response with named viewports and
/// content size. CSS pixel values reflect the post-zoom state; device-pixel
/// counterparts are exposed via [`LayoutMetrics::as_inner`] when needed.
#[derive(Debug, Clone)]
pub struct LayoutMetrics {
    inner: cdp::browser_protocol::page::GetLayoutMetricsResponse,
}

impl LayoutMetrics {
    /// Layout viewport in CSS pixels (the area available for fixed-position elements).
    pub fn css_layout_viewport(&self) -> &cdp::browser_protocol::page::LayoutViewport {
        &self.inner.css_layout_viewport
    }

    /// Visual viewport in CSS pixels (the part of the layout viewport currently visible).
    pub fn css_visual_viewport(&self) -> &cdp::browser_protocol::page::VisualViewport {
        &self.inner.css_visual_viewport
    }

    /// Content size in CSS pixels — the full scrollable area.
    pub fn css_content_size(&self) -> layout::BoundingBox {
        let r = &self.inner.css_content_size;
        layout::BoundingBox::new(r.x, r.y, r.width, r.height)
    }

    /// Borrow the full CDP response for device-pixel viewports and other
    /// fields not surfaced here.
    pub fn as_inner(&self) -> &cdp::browser_protocol::page::GetLayoutMetricsResponse {
        &self.inner
    }

    /// Consume the wrapper and return the full CDP response.
    pub fn into_inner(self) -> cdp::browser_protocol::page::GetLayoutMetricsResponse {
        self.inner
    }
}

impl From<cdp::browser_protocol::page::GetLayoutMetricsResponse> for LayoutMetrics {
    fn from(inner: cdp::browser_protocol::page::GetLayoutMetricsResponse) -> Self {
        LayoutMetrics { inner }
    }
}

/// Identifier for a script registered via [`Page::add_init_script`].
///
/// Pass back to [`Page::remove_init_script`] to unregister the script. The
/// inner string is the CDP `ScriptIdentifier` and is opaque — treat as a token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InitScriptId(pub(crate) cdp::browser_protocol::page::ScriptIdentifier);

/// Per-frame performance metrics returned by [`Page::metrics`].
///
/// Wraps the flat `Vec<cdp_perf::Metric>` returned by `Performance.getMetrics`
/// as a name → value map. Use [`PerfMetrics::get`] for keyed access or
/// [`PerfMetrics::iter`] to enumerate.
#[derive(Debug, Clone, Default)]
pub struct PerfMetrics {
    inner: std::collections::HashMap<String, f64>,
}

impl PerfMetrics {
    /// Look up a metric by its CDP name (e.g. `"JSHeapUsedSize"`).
    pub fn get(&self, name: &str) -> Option<f64> {
        self.inner.get(name).copied()
    }

    /// Iterate `(name, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, f64)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Number of metrics reported.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if no metrics were reported.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl From<Vec<cdp::browser_protocol::performance::Metric>> for PerfMetrics {
    fn from(metrics: Vec<cdp::browser_protocol::performance::Metric>) -> Self {
        Self { inner: metrics.into_iter().map(|m| (m.name, m.value)).collect() }
    }
}

/// Discriminated payload for the two CDP evaluate flavours.
///
/// `Runtime.evaluate` for bare expressions (`document.title`);
/// `Runtime.callFunctionOn` for function-shaped JS (`function(){…}`,
/// `() => …`). Construct via [`Evaluation::expression`] / [`Evaluation::function`]
/// or the `From` impls. The `#[non_exhaustive]` marker reserves room for
/// future variants without a breaking change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Evaluation {
    /// Wraps a `Runtime.evaluate` parameter set.
    Expression(cdp::js_protocol::runtime::EvaluateParams),
    /// Wraps a `Runtime.callFunctionOn` parameter set.
    Function(cdp::js_protocol::runtime::CallFunctionOnParams),
}

impl Evaluation {
    /// Treat `source` as a bare JavaScript expression (e.g. `document.title`,
    /// `1 + 2`).
    ///
    /// The expression is wrapped in `Runtime.evaluate` and its result is
    /// returned as a remote object. Use this when you know the source is *not*
    /// a function declaration or arrow form.
    pub fn expression(source: impl Into<String>) -> Self {
        Evaluation::Expression(cdp::js_protocol::runtime::EvaluateParams::new(source.into()))
    }

    /// Treat `source` as a JavaScript function (named, anonymous, or arrow form)
    /// to be called via `Runtime.callFunctionOn`.
    ///
    /// Use this for `function () { … }`, `async () => …`, etc. When the source
    /// is not actually a function the call will fail at runtime with a
    /// JavaScript exception — prefer this constructor only when the variant is
    /// known statically.
    pub fn function(source: impl Into<String>) -> Self {
        Evaluation::Function(cdp::js_protocol::runtime::CallFunctionOnParams::from(
            source.into().as_str(),
        ))
    }
}

impl From<cdp::js_protocol::runtime::EvaluateParams> for Evaluation {
    fn from(params: cdp::js_protocol::runtime::EvaluateParams) -> Self {
        Evaluation::Expression(params)
    }
}

impl From<cdp::js_protocol::runtime::CallFunctionOnParams> for Evaluation {
    fn from(params: cdp::js_protocol::runtime::CallFunctionOnParams) -> Self {
        Evaluation::Function(params)
    }
}

/// Dispatch an [`EventFrame`] to one or more typed handlers by CDP method name.
///
/// Each arm deserializes the frame's `params` into the specified type and calls
/// the corresponding closure only when the method string matches. Unrecognised
/// methods are silently skipped.
///
/// For single-type matches, prefer [`EventFrame::try_decode`] /
/// [`EventFrame::is`] — they are IDE-discoverable, type-checked, and surface
/// deserialization errors instead of swallowing them.
///
/// ```ignore
/// use chromist::{consume_event, EventFrame};
///
/// while let Some(frame) = sub.next().await {
///     consume_event!(frame, {
///         cdp::browser_protocol::page::LoadEventFiredEvent => |ev| {
///             println!("page loaded");
///         },
///         cdp::browser_protocol::network::ResponseReceivedEvent => |ev| {
///             println!("response: {}", ev.response.url);
///         },
///     });
/// }
/// ```
#[macro_export]
macro_rules! consume_event {
    ($frame:expr, { $($ty:ty => $handler:expr),* $(,)? }) => {{
        let _frame: &$crate::EventFrame = &*$frame;
        $(
            if _frame.method.as_str() == <$ty as $crate::types::MethodType>::method_id().as_ref() {
                if let Ok(_event) = <$ty as serde::Deserialize>::deserialize(&_frame.params) {
                    ($handler)(_event);
                }
            }
        )*
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_constructor_routes_to_expression_variant() {
        assert!(matches!(Evaluation::expression("document.title"), Evaluation::Expression(_)));
    }

    #[test]
    fn function_constructor_routes_to_function_variant() {
        assert!(matches!(
            Evaluation::function("function foo() { return 1; }"),
            Evaluation::Function(_)
        ));
    }

    #[test]
    fn consume_event_matches_by_method_name() {
        use cdp::browser_protocol::page::LoadEventFiredEvent;
        use std::sync::Arc;
        let frame = Arc::new(EventFrame {
            method: "Page.loadEventFired".into(),
            params: serde_json::json!({ "timestamp": 0.0 }),
            session_id: None,
        });
        let hit = std::cell::Cell::new(false);
        consume_event!(&frame, {
            LoadEventFiredEvent => |_ev| { hit.set(true); },
        });
        assert!(hit.get(), "handler for matching method should fire");
    }

    #[test]
    fn consume_event_ignores_non_matching_method() {
        use cdp::browser_protocol::page::LoadEventFiredEvent;
        use std::sync::Arc;
        let frame = Arc::new(EventFrame {
            method: "Network.requestWillBeSent".into(),
            params: serde_json::json!({}),
            session_id: None,
        });
        let hit = std::cell::Cell::new(false);
        consume_event!(&frame, {
            LoadEventFiredEvent => |_ev| { hit.set(true); },
        });
        assert!(!hit.get(), "handler must not fire on unrelated method");
    }
}
