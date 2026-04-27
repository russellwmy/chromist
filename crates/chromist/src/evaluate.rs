use crate::cdp::js_protocol::runtime as cdp_runtime;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Wrapper around a CDP `Runtime.evaluate` response.
///
/// Returned by [`Page::evaluate`](crate::Page::evaluate) and friends. Use
/// [`Self::into_value`] to deserialize into a typed value, [`Self::value`]
/// for a borrowed JSON view, or [`Self::is_truthy`] for a JavaScript-style
/// truthiness check.
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    pub(crate) inner: cdp_runtime::EvaluateResponse,
}

impl EvaluationResult {
    /// Consume the result and deserialize into `T` via `serde_json`.
    ///
    /// Returns `serde_json::Error` if the runtime value cannot be coerced
    /// into `T` — typically when the JS expression returned a value of
    /// unexpected shape.
    pub fn into_value<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.inner.result.value.clone().unwrap_or(Value::Null))
    }

    /// Borrow the underlying JSON value, if the expression returned one.
    /// `None` for primitive results that the browser did not include
    /// (e.g. when `return_by_value` was false).
    pub fn value(&self) -> Option<&Value> {
        self.inner.result.value.as_ref()
    }

    /// Borrow the full CDP `RemoteObject` payload, including the type tag,
    /// subtype, description, and (for non-primitives) the live `objectId`.
    pub fn object(&self) -> &cdp_runtime::RemoteObject {
        &self.inner.result
    }

    /// Consume the wrapper and return the raw CDP `EvaluateResponse` —
    /// useful when you need fields not exposed by the wrapper.
    pub fn into_inner(self) -> cdp_runtime::EvaluateResponse {
        self.inner
    }

    /// Returns `true` if the result is truthy by JavaScript semantics.
    ///
    /// Falsy values: `null`, `undefined`, `false`, `0`, `""`.
    /// Everything else (objects, non-zero numbers, non-empty strings, `true`) is truthy.
    pub fn is_truthy(&self) -> bool {
        use crate::cdp::js_protocol::runtime::RemoteObjectType;
        match &self.inner.result.r#type {
            RemoteObjectType::Undefined => false,
            RemoteObjectType::Boolean => {
                self.inner.result.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false)
            }
            RemoteObjectType::Number => {
                self.inner.result.value.as_ref().and_then(|v| v.as_f64()).is_some_and(|n| n != 0.0)
            }
            RemoteObjectType::String => self
                .inner
                .result
                .value
                .as_ref()
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty()),
            RemoteObjectType::Object => {
                // CDP represents `null` as type=object + subtype=null
                let is_null = self
                    .inner
                    .result
                    .subtype
                    .as_ref()
                    .map(|s| s.as_ref() == "null")
                    .unwrap_or(false);
                !is_null
            }
            // function, symbol, bigint — always truthy
            _ => true,
        }
    }
}

/// Returns true if the JS string looks like a function declaration (not a bare expression).
pub(crate) fn is_likely_js_function(js: &str) -> bool {
    let t = js.trim();
    t.starts_with("function")
        || t.starts_with("async function")
        || t.starts_with("async (")
        || (t.starts_with('(') && t.contains("=>"))
        || (t.starts_with("/*") && t.contains("=>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_named_function() {
        assert!(is_likely_js_function("function foo() { return 1; }"));
    }

    #[test]
    fn recognises_async_function() {
        assert!(is_likely_js_function("async function f(){}"));
    }

    #[test]
    fn recognises_async_arrow() {
        assert!(is_likely_js_function("async () => 1"));
    }

    #[test]
    fn recognises_arrow_with_parens() {
        assert!(is_likely_js_function("(a, b) => a + b"));
    }

    #[test]
    fn recognises_block_comment_then_arrow() {
        assert!(is_likely_js_function("/* doc */ (x) => x"));
    }

    #[test]
    fn rejects_bare_expression() {
        assert!(!is_likely_js_function("document.title"));
        assert!(!is_likely_js_function("1 + 2"));
        assert!(!is_likely_js_function("window.location.href"));
    }

    #[test]
    fn rejects_arrow_without_leading_paren() {
        // A single-arg arrow like `x => x` is not matched by the heuristic.
        // This documents current behaviour.
        assert!(!is_likely_js_function("x => x + 1"));
    }

    #[test]
    fn handles_leading_whitespace() {
        assert!(is_likely_js_function("   function foo(){}"));
        assert!(is_likely_js_function("\n\tasync function x(){}"));
    }

    #[test]
    fn empty_string_is_not_function() {
        assert!(!is_likely_js_function(""));
        assert!(!is_likely_js_function("   "));
    }
}
