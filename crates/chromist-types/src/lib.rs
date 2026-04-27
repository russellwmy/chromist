//! Core CDP wire types and traits for chromist.
//!
//! This crate provides the foundational types used to communicate over the
//! Chrome DevTools Protocol (CDP): method call/response envelopes, error
//! types, event messages, and the trait abstractions that generated code
//! implements.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// BuildError
// ---------------------------------------------------------------------------

/// Error returned by generated builder `build()` methods when a mandatory
/// field was not set before calling `build()`.
#[non_exhaustive]
#[derive(Debug)]
pub struct BuildError {
    /// Name of the mandatory field that was not set before calling `build()`.
    pub field: &'static str,
}

impl BuildError {
    /// Construct a `BuildError` naming the missing field.
    #[must_use]
    pub const fn new(field: &'static str) -> Self {
        BuildError { field }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mandatory field `{}` was not set", self.field)
    }
}

impl std::error::Error for BuildError {}

// ---------------------------------------------------------------------------
// Primitive aliases
// ---------------------------------------------------------------------------

/// A CDP method identifier, e.g. `"Page.navigate"`.
///
/// Using [`Cow<'static, str>`] lets generated code embed `&'static str`
/// literals without any heap allocation while still allowing owned strings
/// where needed.
pub type MethodId = Cow<'static, str>;

// ---------------------------------------------------------------------------
// CallId
// ---------------------------------------------------------------------------

/// Monotonically increasing identifier that correlates a [`MethodCall`] with
/// its [`Response`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CallId(usize);

impl CallId {
    /// Create a new `CallId` from a raw counter value.
    #[must_use]
    pub const fn new(id: usize) -> Self {
        CallId(id)
    }

    /// Return the underlying counter value.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<usize> for CallId {
    fn from(v: usize) -> Self {
        CallId(v)
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// An outbound CDP command sent to the browser.
#[derive(Debug, Clone, Serialize)]
pub struct MethodCall {
    /// Correlation ID; matched against the `id` field of the browser's response.
    pub id: CallId,
    /// Fully-qualified CDP method name, e.g. `"Page.navigate"`.
    pub method: MethodId,
    /// Session ID for multi-target sessions; omitted from the wire when `None`.
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Arc<str>>,
    /// Method-specific parameters serialized as a JSON object.
    pub params: serde_json::Value,
}

/// An error returned by the browser in response to a [`MethodCall`].
///
/// Named `CdpError` to avoid collision with [`std::error::Error`].
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct CdpError {
    /// JSON-RPC error code (negative integer; e.g. `-32601` for method-not-found).
    pub code: i64,
    /// Human-readable error message from the browser.
    pub message: String,
}

impl CdpError {
    /// Construct a `CdpError` from a JSON-RPC error code and message.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        CdpError { code, message: message.into() }
    }
}

impl fmt::Display for CdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CDP error {} — {}", self.code, self.message)
    }
}

impl std::error::Error for CdpError {}

/// An inbound CDP response envelope.
///
/// Exactly one of `result` or `error` is `Some` in a well-formed response.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    /// Correlation ID matching the originating [`MethodCall::id`].
    pub id: CallId,
    /// Successful result payload; `None` when `error` is `Some`.
    pub result: Option<serde_json::Value>,
    /// Error payload; `None` when `result` is `Some`.
    pub error: Option<CdpError>,
}

/// A raw inbound CDP event.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpJsonEventMessage {
    /// Fully-qualified event name, e.g. `"Network.responseReceived"`.
    pub method: String,
    /// Event-specific payload as a raw JSON object.
    pub params: serde_json::Value,
    /// Session ID for multi-target sessions; `None` for browser-level events.
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<Arc<str>>,
}

/// Top-level envelope for any message arriving from the browser.
///
/// Uses `#[serde(untagged)]` so that serde tries [`Response`] first (it has
/// an `"id"` field) and falls back to `T` (the event type).
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Message<T = CdpJsonEventMessage> {
    Response(Box<Response>),
    Event(T),
}

/// A typed command response produced by [`chromist`'s handler] after a
/// round-trip CDP call.  It pairs the correlation [`CallId`] with the decoded
/// result and the optional session that scoped the call.
///
/// This type lives in `chromist-types` so that the handler and the transport
/// layer share the same definition without depending on the full `chromist`
/// crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse<T> {
    /// Correlation ID matching the originating call.
    pub id: CallId,
    /// Decoded result payload.
    pub result: T,
    /// Session ID that scoped the original call, if any.
    pub session_id: Option<Arc<str>>,
}

// ---------------------------------------------------------------------------
// Binary (base64 newtype)
// ---------------------------------------------------------------------------

/// A byte buffer that serializes as a base64-encoded string over the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binary(Vec<u8>);

impl Binary {
    /// Wrap a byte vector as a `Binary`.
    #[must_use]
    pub const fn from_bytes(bytes: Vec<u8>) -> Self {
        Binary(bytes)
    }

    /// Borrow the raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Unwrap into the inner byte vector.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl Serialize for Binary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(self.as_bytes());
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64::Engine as _;
        let s = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)?;
        Ok(Binary::from_bytes(bytes))
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Any type that represents a CDP method (command or event).
pub trait Method {
    /// The fully-qualified method identifier, e.g. `"Page.navigate"`.
    fn identifier(&self) -> MethodId;

    /// The domain portion of the identifier (everything before the first `.`).
    ///
    /// Returns `None` when the identifier is not a static string (e.g. for
    /// dynamically-constructed event messages). Generated code always produces
    /// `'static` identifiers and will always return `Some`.
    fn domain(&self) -> Option<&str> {
        match self.identifier() {
            Cow::Borrowed(s) => Some(s.split('.').next().unwrap_or(s)),
            Cow::Owned(_) => None,
        }
    }

    /// The method-name portion of the identifier (everything after the first `.`).
    ///
    /// Returns `None` when the identifier is not a static string. Generated
    /// code always produces `'static` identifiers and will always return `Some`.
    fn method_name(&self) -> Option<&str> {
        match self.identifier() {
            Cow::Borrowed(s) => Some(s.split_once('.').map(|x| x.1).unwrap_or(s)),
            Cow::Owned(_) => None,
        }
    }
}

/// Provides the static method identifier for a type (usable without an instance).
///
/// # Example
///
/// ```
/// use chromist_types::{MethodId, MethodType};
/// use std::borrow::Cow;
///
/// struct MyEvent;
/// impl MethodType for MyEvent {
///     fn method_id() -> MethodId { Cow::Borrowed("Domain.myEvent") }
/// }
///
/// assert_eq!(MyEvent::method_id(), "Domain.myEvent");
/// ```
pub trait MethodType {
    /// Returns the fully-qualified method identifier for this type.
    fn method_id() -> MethodId
    where
        Self: Sized;
}

/// A CDP command: something that can be serialized and sent to the browser,
/// and whose response can be deserialized.
pub trait Command: Serialize + Method {
    /// The type of the successful response payload.
    type Response: serde::de::DeserializeOwned;
}

/// A CDP event message: something that arrives unsolicited from the browser.
pub trait EventMessage: Method + serde::de::DeserializeOwned {}

impl Method for CdpJsonEventMessage {
    fn identifier(&self) -> MethodId {
        Cow::Owned(self.method.clone())
    }
}

impl EventMessage for CdpJsonEventMessage {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_call_serializes() {
        let call = MethodCall {
            id: CallId::new(1),
            method: Cow::Borrowed("Page.navigate"),
            session_id: None,
            params: serde_json::json!({ "url": "https://example.com" }),
        };
        let json = serde_json::to_value(&call).expect("serialize");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "Page.navigate");
        assert!(json.get("session_id").is_none(), "session_id should be omitted when None");
        assert!(json.get("sessionId").is_none(), "sessionId should be omitted when None");
    }

    #[test]
    fn method_call_session_id_uses_camel_case_wire_name() {
        let call = MethodCall {
            id: CallId::new(1),
            method: Cow::Borrowed("Page.navigate"),
            session_id: Some(Arc::from("abc123")),
            params: serde_json::json!({ "url": "https://example.com" }),
        };
        let json = serde_json::to_value(&call).expect("serialize");
        assert_eq!(json["sessionId"], "abc123");
        assert!(json.get("session_id").is_none(), "wire name must be camelCase sessionId");
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        for k in &keys {
            assert!(
                matches!(*k, "id" | "method" | "sessionId" | "params"),
                "unexpected top-level key {k}: CDP rejects anything outside this set"
            );
        }
    }

    #[test]
    fn response_result_xor_error() {
        // A well-formed CDP response has exactly one of result or error.
        let ok_raw = r#"{"id":1,"result":{"value":42}}"#;
        let ok: Response = serde_json::from_str(ok_raw).unwrap();
        assert!(
            ok.result.is_some() && ok.error.is_none(),
            "ok response must have result, no error"
        );

        let err_raw = r#"{"id":2,"error":{"code":-32000,"message":"oops"}}"#;
        let err: Response = serde_json::from_str(err_raw).unwrap();
        assert!(
            err.result.is_none() && err.error.is_some(),
            "error response must have error, no result"
        );

        // Both None is technically representable but never produced by Chrome.
        let both_none_raw = r#"{"id":3}"#;
        let both_none: Response = serde_json::from_str(both_none_raw).unwrap();
        assert!(both_none.result.is_none() && both_none.error.is_none());
    }

    #[test]
    fn response_deserializes_ok() {
        let raw = r#"{"id":1,"result":{"value":42}}"#;
        let resp: Response = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(resp.id, CallId::new(1));
        assert!(resp.error.is_none());
        let result = resp.result.expect("result present");
        assert_eq!(result["value"], 42);
    }

    #[test]
    fn response_deserializes_error() {
        let raw = r#"{"id":2,"error":{"code":-32000,"message":"oops"}}"#;
        let resp: Response = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(resp.id, CallId::new(2));
        let err = resp.error.expect("error present");
        assert_eq!(err.code, -32000);
        assert_eq!(err.message, "oops");
    }

    #[test]
    fn message_untagged_routes_response() {
        let raw = r#"{"id":1,"result":{}}"#;
        let msg: Message = serde_json::from_str(raw).expect("deserialize");
        assert!(matches!(msg, Message::Response(_)));
    }

    #[test]
    fn message_untagged_routes_event() {
        let raw = r#"{"method":"Page.loadEventFired","params":{}}"#;
        let msg: Message = serde_json::from_str(raw).expect("deserialize");
        assert!(matches!(msg, Message::Event(_)));
    }

    #[test]
    fn binary_roundtrip() {
        let original = Binary::from_bytes(b"hello".to_vec());
        let serialized = serde_json::to_string(&original).expect("serialize");
        let deserialized: Binary = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized.as_bytes(), b"hello");
    }

    #[test]
    fn binary_roundtrip_edge_cases() {
        for bytes in [Vec::new(), vec![0u8], vec![0xff], (0u8..=255).collect(), vec![0u8; 1024]] {
            let original = Binary::from_bytes(bytes.clone());
            let serialized = serde_json::to_string(&original).expect("serialize");
            let deserialized: Binary = serde_json::from_str(&serialized).expect("deserialize");
            assert_eq!(deserialized.as_bytes(), bytes.as_slice());
        }
    }

    #[test]
    fn binary_decode_rejects_invalid_base64() {
        let result: Result<Binary, _> = serde_json::from_str(r#""not!valid!base64!""#);
        assert!(result.is_err(), "invalid base64 must fail deserialize");
    }

    #[test]
    fn build_error_display() {
        let err = BuildError::new("target_id");
        assert_eq!(err.to_string(), "mandatory field `target_id` was not set");
    }

    #[test]
    fn cdp_error_constructor() {
        let err = CdpError::new(-32601, "method not found");
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "method not found");
    }

    struct PageNavigate;
    impl Method for PageNavigate {
        fn identifier(&self) -> MethodId {
            Cow::Borrowed("Page.navigate")
        }
    }

    #[test]
    fn test_method_domain() {
        assert_eq!(PageNavigate.domain(), Some("Page"));
    }

    #[test]
    fn test_method_name() {
        assert_eq!(PageNavigate.method_name(), Some("navigate"));
    }

    #[test]
    fn test_call_id_ordering() {
        assert!(CallId::new(1) < CallId::new(2));
        assert!(CallId::new(10) > CallId::new(5));
        assert_eq!(CallId::new(3), CallId::new(3));
    }

    #[test]
    fn call_id_from_usize() {
        let id = CallId::from(7usize);
        assert_eq!(id.get(), 7);
    }

    #[test]
    fn method_without_dot_falls_back_to_full_identifier() {
        struct NoDot;
        impl Method for NoDot {
            fn identifier(&self) -> MethodId {
                Cow::Borrowed("bareMethod")
            }
        }
        assert_eq!(NoDot.domain(), Some("bareMethod"));
        assert_eq!(NoDot.method_name(), Some("bareMethod"));
    }

    #[test]
    fn owned_identifier_returns_none_for_domain() {
        struct DynEvent(String);
        impl Method for DynEvent {
            fn identifier(&self) -> MethodId {
                Cow::Owned(self.0.clone())
            }
        }
        assert_eq!(DynEvent("Page.navigate".into()).domain(), None);
        assert_eq!(DynEvent("Page.navigate".into()).method_name(), None);
    }

    // -----------------------------------------------------------------------
    // Property tests
    //
    // `domain` and `method_name` on the `Method` trait split on '.'.
    // These tests prove the splitting logic never panics on arbitrary input and
    // that `CallId`'s derived `Ord` is a total order.
    // -----------------------------------------------------------------------

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn domain_split_never_panics(s in ".*") {
            // Mirrors the Cow::Borrowed arm: split('.').next() or full str.
            let _domain = s.split('.').next().unwrap_or(&s);
            // Mirrors method_name: split_once('.').map(|x| x.1) or full str.
            let _name = s.split_once('.').map(|x| x.1).unwrap_or(&s);
        }

        #[test]
        fn owned_identifier_always_returns_none(s in ".*") {
            struct Dyn(String);
            impl Method for Dyn {
                fn identifier(&self) -> MethodId { Cow::Owned(self.0.clone()) }
            }
            let d = Dyn(s);
            prop_assert!(d.domain().is_none());
            prop_assert!(d.method_name().is_none());
        }

        #[test]
        fn call_id_ord_is_total(a in 0usize..usize::MAX, b in 0usize..usize::MAX) {
            let ca = CallId::new(a);
            let cb = CallId::new(b);
            let lt = ca < cb;
            let eq = ca == cb;
            let gt = ca > cb;
            prop_assert_eq!(
                lt as u8 + eq as u8 + gt as u8,
                1u8,
                "exactly one ordering must hold"
            );
        }
    }
}
