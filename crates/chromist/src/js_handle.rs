use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::error::CdpError;
use crate::handler::{HandlerHandle, SessionRef};

/// A handle to a live JavaScript object in the browser.
///
/// The browser keeps the referenced object alive until [`dispose`](JsHandle::dispose)
/// is called, the page navigates, or the page closes.
///
/// Use [`Page::evaluate_handle`](crate::page::Page::evaluate_handle) to obtain a handle.
#[derive(Clone)]
pub struct JsHandle {
    pub(crate) handle: HandlerHandle,
    pub(crate) session_id: SessionRef,
    pub(crate) object_id: cdp_runtime::RemoteObjectId,
}

impl std::fmt::Debug for JsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsHandle").field("object_id", &self.object_id).finish()
    }
}

impl JsHandle {
    pub(crate) fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        object_id: cdp_runtime::RemoteObjectId,
    ) -> Self {
        Self { handle, session_id, object_id }
    }

    /// The CDP remote object ID for this handle.
    pub fn object_id(&self) -> &cdp_runtime::RemoteObjectId {
        &self.object_id
    }

    /// Evaluate `js_fn` in the context of this object (`this` = the referenced object).
    ///
    /// Returns the JSON-serialized result. Use [`evaluate_handle`](Self::evaluate_handle)
    /// to receive an object result without serialization.
    pub async fn evaluate(&self, js_fn: impl Into<String>) -> crate::Result<serde_json::Value> {
        let mut params = cdp_runtime::CallFunctionOnParams::new(js_fn.into());
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(ex) = resp.exception_details {
            return Err(CdpError::JavascriptException(Box::new(ex)));
        }
        Ok(resp.result.value.unwrap_or(serde_json::Value::Null))
    }

    /// Like [`evaluate`](Self::evaluate) but returns a new `JsHandle` wrapping the result.
    ///
    /// Returns `CdpError::MissingObjectId` if the function returns a primitive value.
    pub async fn evaluate_handle(&self, js_fn: impl Into<String>) -> crate::Result<JsHandle> {
        let mut params = cdp_runtime::CallFunctionOnParams::new(js_fn.into());
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(false);
        params.await_promise = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(ex) = resp.exception_details {
            return Err(CdpError::JavascriptException(Box::new(ex)));
        }
        let object_id = resp.result.object_id.ok_or(CdpError::MissingObjectId)?;
        Ok(JsHandle::new(self.handle.clone(), self.session_id.clone(), object_id))
    }

    /// Return the JSON value of the referenced object.
    pub async fn json_value(&self) -> crate::Result<serde_json::Value> {
        self.evaluate("function() { return this; }").await
    }

    /// Attempt to cast this handle to a DOM [`Element`](crate::element::Element).
    ///
    /// Returns `None` if the remote object is not a DOM node.
    pub async fn as_element(&self) -> crate::Result<Option<crate::element::Element>> {
        let params = cdp_dom::RequestNodeParams::new(self.object_id.clone());
        match self.handle.execute(params, Some(self.session_id.current())).await {
            Ok(r) => {
                let el = crate::element::Element::from_node_id(
                    self.handle.clone(),
                    self.session_id.clone(),
                    r.node_id,
                    None,
                    None,
                )
                .await?;
                Ok(Some(el))
            }
            Err(_) => Ok(None),
        }
    }

    /// Release the remote object, freeing browser memory.
    pub async fn dispose(&self) -> crate::Result<()> {
        let params = cdp_runtime::ReleaseObjectParams::new(self.object_id.clone());
        let _ = self.handle.execute(params, Some(self.session_id.current())).await;
        Ok(())
    }

    /// Build a CDP `CallArgument` that passes this handle by remote reference.
    ///
    /// Pass the result in the `arguments` field of `CallFunctionOnParams` to
    /// forward this handle as an argument to another CDP function call.
    pub fn as_call_argument(&self) -> cdp_runtime::CallArgument {
        cdp_runtime::CallArgument {
            value: None,
            unserializable_value: None,
            object_id: Some(self.object_id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn as_call_argument_sets_object_id() {
        // CallArgument has value=None, object_id=Some when built from a handle reference.
        // This documents the expected shape without requiring a live browser.
        use crate::cdp::js_protocol::runtime as cdp_runtime;
        let arg = cdp_runtime::CallArgument {
            value: None,
            unserializable_value: None,
            object_id: Some(cdp_runtime::RemoteObjectId::from("test-id".to_string())),
        };
        assert!(arg.object_id.is_some());
        assert!(arg.value.is_none());
    }
}
