//! Round-trip serde tests for generated CDP types.
//!
//! Each test deserializes a JSON fixture that matches what Chrome actually
//! sends, then re-serializes and checks the result.

use chromist_cdp::cdp;

#[test]
fn page_navigate_params_roundtrip() {
    let params = cdp::browser_protocol::page::NavigateParams::new("https://example.com");
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["url"], "https://example.com");
    let back: cdp::browser_protocol::page::NavigateParams = serde_json::from_value(json).unwrap();
    assert_eq!(back.url, "https://example.com");
}

#[test]
fn network_response_received_event_deserializes() {
    let raw = r#"{
        "requestId": "123",
        "loaderId": "loader1",
        "timestamp": 1.5,
        "type": "Document",
        "response": {
            "url": "https://example.com",
            "status": 200,
            "statusText": "OK",
            "headers": {},
            "mimeType": "text/html",
            "charset": "utf-8",
            "connectionReused": false,
            "connectionId": 0,
            "encodedDataLength": 0,
            "securityState": "secure"
        },
        "frameId": "frame1",
        "hasExtraInfo": false
    }"#;
    let ev: cdp::browser_protocol::network::ResponseReceivedEvent =
        serde_json::from_str(raw).expect("deserialize ResponseReceived");
    assert_eq!(ev.request_id.as_ref(), "123");
    assert_eq!(ev.response.status, 200);
}

#[test]
fn runtime_evaluate_params_roundtrip() {
    use cdp::js_protocol::runtime::EvaluateParams;
    let mut params = EvaluateParams::new("1+1");
    params.return_by_value = Some(true);
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["expression"], "1+1");
    assert_eq!(json["returnByValue"], true);
    let back: EvaluateParams = serde_json::from_value(json).unwrap();
    assert_eq!(back.expression, "1+1");
    assert_eq!(back.return_by_value, Some(true));
}

#[test]
fn cdp_event_message_deserializes_page_load() {
    use cdp::events::{CdpEvent, CdpEventMessage};
    let raw = r#"{"method":"Page.loadEventFired","params":{"timestamp":1.23}}"#;
    let msg: CdpEventMessage = serde_json::from_str(raw).unwrap();
    assert!(matches!(msg.params, CdpEvent::PageLoadEventFired(_)));
}

#[test]
fn target_create_browser_context_builder_errors_on_missing_field() {
    use chromist_types::BuildError;
    // `SetDiscoverTargetsParams.discover` is mandatory; builder should error.
    let result = cdp::browser_protocol::target::SetDiscoverTargetsParamsBuilder::default().build();
    let _: BuildError = result.expect_err("missing mandatory field should fail");
}

#[test]
fn command_identifier_is_static_str() {
    use chromist_types::{Method, MethodType};
    let params = cdp::browser_protocol::page::NavigateParams::new("https://example.com");
    assert_eq!(params.identifier(), "Page.navigate");
    assert_eq!(cdp::browser_protocol::page::NavigateParams::method_id(), "Page.navigate");
}

#[test]
fn binary_field_roundtrips_as_base64() {
    use cdp::browser_protocol::bluetooth_emulation::ManufacturerData;
    use chromist_types::Binary;

    let payload = vec![0xde, 0xad, 0xbe, 0xef];
    let original = ManufacturerData::new(0x1234_i64, Binary::from_bytes(payload.clone()));

    let json = serde_json::to_value(&original).expect("serialize");
    // Binary must be encoded as a base64 string, not a JSON array of numbers.
    assert!(json["data"].is_string(), "Binary must serialize to a string");
    assert_eq!(json["data"].as_str().unwrap(), "3q2+7w==");

    let back: ManufacturerData = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.key, 0x1234);
    assert_eq!(back.data.as_bytes(), payload.as_slice());
}

#[test]
fn global_event_enum_deserializes_target_attached() {
    use cdp::events::{CdpEvent, CdpEventMessage};
    let raw = r#"{"method":"Target.attachedToTarget","params":{
        "sessionId":"sess-1",
        "targetInfo":{
            "targetId":"t-1",
            "type":"page",
            "title":"t",
            "url":"about:blank",
            "attached":true,
            "canAccessOpener":false
        },
        "waitingForDebugger":false
    }}"#;
    let msg: CdpEventMessage = serde_json::from_str(raw).unwrap();
    assert!(matches!(msg.params, CdpEvent::TargetAttachedToTarget(_)));
}

// --- Error-path tests ---

#[test]
fn missing_required_field_returns_error() {
    // NavigateParams requires "url"; omitting it must produce a serde error, not panic.
    let json = serde_json::json!({});
    let result: Result<cdp::browser_protocol::page::NavigateParams, _> =
        serde_json::from_value(json);
    assert!(result.is_err(), "missing required field must fail");
}

#[test]
fn wrong_type_for_field_returns_error() {
    // "url" must be a string; passing an integer must fail.
    let json = serde_json::json!({ "url": 42 });
    let result: Result<cdp::browser_protocol::page::NavigateParams, _> =
        serde_json::from_value(json);
    assert!(result.is_err(), "wrong type for required string field must fail");
}

#[test]
fn unknown_event_method_falls_back_to_other() {
    use cdp::events::{CdpEvent, CdpEventMessage};
    // An unrecognized method name must deserialize as CdpEvent::Other, not error.
    let raw = r#"{"method":"Nonexistent.unknownEvent","params":{"foo":"bar"}}"#;
    let msg: CdpEventMessage =
        serde_json::from_str(raw).expect("unknown event method must still deserialize");
    assert!(
        matches!(msg.params, CdpEvent::Other(_)),
        "unrecognized event must map to CdpEvent::Other"
    );
}

#[test]
fn malformed_json_returns_error() {
    // Truncated JSON must produce a serde error.
    let raw = r#"{"method":"Page.loadEventFired","params":{"timestamp":"#;
    let result: Result<cdp::events::CdpEventMessage, _> = serde_json::from_str(raw);
    assert!(result.is_err(), "truncated JSON must fail");
}

#[test]
fn network_request_will_be_sent_roundtrip() {
    // Cover a high-traffic event: Network.requestWillBeSent
    let raw = r#"{
        "requestId": "req-1",
        "loaderId": "load-1",
        "documentURL": "https://example.com",
        "request": {
            "url": "https://example.com/api",
            "method": "GET",
            "headers": {},
            "initialPriority": "VeryHigh",
            "referrerPolicy": "strict-origin-when-cross-origin"
        },
        "timestamp": 1.0,
        "wallTime": 2.0,
        "initiator": { "type": "other" },
        "redirectHasExtraInfo": false,
        "type": "XHR",
        "frameId": "frame-1"
    }"#;
    let ev: cdp::browser_protocol::network::RequestWillBeSentEvent =
        serde_json::from_str(raw).expect("requestWillBeSent must deserialize");
    assert_eq!(ev.request_id.as_ref(), "req-1");
    assert_eq!(ev.request.method, "GET");
    assert_eq!(ev.request.url, "https://example.com/api");
}

#[test]
fn response_received_missing_required_nested_field_returns_error() {
    // response.url is required; omitting the entire response object must fail.
    let raw = r#"{
        "requestId": "req-2",
        "loaderId": "load-2",
        "timestamp": 1.0,
        "type": "Document",
        "hasExtraInfo": false
    }"#;
    let result: Result<cdp::browser_protocol::network::ResponseReceivedEvent, _> =
        serde_json::from_str(raw);
    assert!(result.is_err(), "missing response object must fail");
}

#[test]
fn invalid_base64_in_binary_field_returns_error() {
    use cdp::browser_protocol::bluetooth_emulation::ManufacturerData;
    // "data" must be valid base64; garbage must fail.
    let json = serde_json::json!({ "key": 1, "data": "not!!valid!!base64@@" });
    let result: Result<ManufacturerData, _> = serde_json::from_value(json);
    assert!(result.is_err(), "invalid base64 must fail");
}
