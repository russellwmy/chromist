//! JavaScript dialog (`alert`, `confirm`, `prompt`, `beforeunload`) handling.
//!
//! Subscribe to dialog events via [`Page::dialog_stream`](crate::Page::dialog_stream)
//! or [`Page::on_dialog`](crate::Page::on_dialog). Each yielded event can be
//! wrapped in a [`DialogHandler`] (constructed by the caller from the event and
//! the originating page) to respond with [`DialogHandler::accept`] /
//! [`DialogHandler::dismiss`]. Or, if you don't need per-event state, call the
//! helpers on `Page` directly: [`Page::accept_dialog`](crate::Page::accept_dialog),
//! [`Page::dismiss_dialog`](crate::Page::dismiss_dialog).

use crate::cdp::browser_protocol::page::{
    self as cdp_page, HandleJavaScriptDialogParams, JavascriptDialogOpeningEvent,
};
use crate::handler::{HandlerHandle, SessionRef};

/// JavaScript dialog type.
///
/// Mirrors the CDP `Page.DialogType` enum, but exposed as a plain enum so
/// callers can match on it without importing the generated CDP type.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogType {
    /// `alert(...)` — single OK button, no return value.
    Alert,
    /// `confirm(...)` — OK / Cancel, returns boolean.
    Confirm,
    /// `prompt(...)` — text input + OK / Cancel, returns string or null.
    Prompt,
    /// `beforeunload` confirmation shown when navigating away from a page
    /// with a `window.onbeforeunload` handler.
    BeforeUnload,
}

impl std::fmt::Display for DialogType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialogType::Alert => f.write_str("alert"),
            DialogType::Confirm => f.write_str("confirm"),
            DialogType::Prompt => f.write_str("prompt"),
            DialogType::BeforeUnload => f.write_str("beforeunload"),
        }
    }
}

impl From<cdp_page::DialogType> for DialogType {
    fn from(d: cdp_page::DialogType) -> Self {
        match d {
            cdp_page::DialogType::Alert => DialogType::Alert,
            cdp_page::DialogType::Confirm => DialogType::Confirm,
            cdp_page::DialogType::Prompt => DialogType::Prompt,
            cdp_page::DialogType::Beforeunload => DialogType::BeforeUnload,
            _ => DialogType::Alert,
        }
    }
}

/// Owned view over a `Page.javascriptDialogOpening` event paired with the
/// handler needed to respond.
#[derive(Debug, Clone)]
pub struct DialogHandler {
    handle: HandlerHandle,
    session_id: SessionRef,
    event: JavascriptDialogOpeningEvent,
}

impl DialogHandler {
    /// Construct a handler from a received dialog event.
    pub fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        event: JavascriptDialogOpeningEvent,
    ) -> Self {
        Self { handle, session_id, event }
    }

    /// Accept the dialog, optionally supplying `prompt_text` for a `prompt()`.
    pub async fn accept(&self, prompt_text: Option<String>) -> crate::Result<()> {
        let mut params = HandleJavaScriptDialogParams::new(true);
        params.prompt_text = prompt_text;
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Dismiss (cancel) the dialog.
    pub async fn dismiss(&self) -> crate::Result<()> {
        let params = HandleJavaScriptDialogParams::new(false);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// The dialog's message text.
    pub fn message(&self) -> &str {
        &self.event.message
    }

    /// Returns the kind of JavaScript dialog (`alert`, `confirm`,
    /// `prompt`, or `beforeunload`).
    pub fn dialog_type(&self) -> DialogType {
        self.event.r#type.clone().into()
    }

    /// The default text shown in a `prompt()` dialog, if any.
    pub fn default_prompt(&self) -> Option<&str> {
        self.event.default_prompt.as_deref()
    }

    /// Whether the browser has a native handler for this dialog type.
    pub fn has_browser_handler(&self) -> bool {
        self.event.has_browser_handler
    }

    /// URL of the frame that opened the dialog.
    pub fn url(&self) -> &str {
        &self.event.url
    }

    /// Access the raw CDP event if finer detail is needed.
    pub fn event(&self) -> &JavascriptDialogOpeningEvent {
        &self.event
    }
}
