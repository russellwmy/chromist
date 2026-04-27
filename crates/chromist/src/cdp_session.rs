//! Raw CDP session access for advanced use cases.

use crate::handler::{HandlerHandle, SessionRef};

/// A raw CDP session handle returned by `Page::new_cdp_session`.
///
/// Allows sending arbitrary CDP commands directly over the page's session.
/// Prefer the typed `Page` methods for normal automation; use this only when
/// you need access to CDP domains not yet surfaced by the high-level API.
#[derive(Clone, Debug)]
pub struct CdpSession {
    handle: HandlerHandle,
    session_id: SessionRef,
}

impl CdpSession {
    pub(crate) fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id }
    }

    /// Send a typed CDP command and return its response.
    pub async fn send<C>(&self, cmd: C) -> crate::Result<C::Response>
    where
        C: chromist_types::Command + serde::Serialize + Send + 'static,
        C::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        self.handle.execute(cmd, Some(self.session_id.current())).await
    }

    /// Returns the current CDP session ID string.
    pub fn session_id(&self) -> std::sync::Arc<str> {
        self.session_id.current()
    }
}
