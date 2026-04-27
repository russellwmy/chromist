//! File chooser interception.
//!
//! Obtain via [`Page::wait_for_file_chooser`](crate::Page::wait_for_file_chooser).
//! The returned [`FileChooser`] lets you call [`accept`](FileChooser::accept)
//! with a list of paths to simulate the user's selection, or
//! [`cancel`](FileChooser::cancel) to dismiss the dialog.

use std::path::PathBuf;

use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::browser_protocol::page::{
    self as cdp_page, FileChooserOpenedEvent, FileChooserOpenedEventMode,
};
use crate::error::CdpError;
use crate::handler::{HandlerHandle, SessionRef};

/// File input mode for an intercepted file chooser.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChooserMode {
    /// `<input type="file">` — accepts exactly one file.
    SelectSingle,
    /// `<input type="file" multiple>` — accepts multiple files.
    SelectMultiple,
}

impl std::fmt::Display for FileChooserMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileChooserMode::SelectSingle => f.write_str("selectSingle"),
            FileChooserMode::SelectMultiple => f.write_str("selectMultiple"),
        }
    }
}

impl From<FileChooserOpenedEventMode> for FileChooserMode {
    fn from(m: FileChooserOpenedEventMode) -> Self {
        match m {
            FileChooserOpenedEventMode::SelectSingle => FileChooserMode::SelectSingle,
            FileChooserOpenedEventMode::SelectMultiple => FileChooserMode::SelectMultiple,
            _ => FileChooserMode::SelectSingle,
        }
    }
}

/// An intercepted file chooser dialog awaiting caller action.
#[derive(Debug, Clone)]
pub struct FileChooser {
    handle: HandlerHandle,
    session_id: SessionRef,
    event: FileChooserOpenedEvent,
}

impl FileChooser {
    pub(crate) fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        event: FileChooserOpenedEvent,
    ) -> Self {
        Self { handle, session_id, event }
    }

    /// `true` if the underlying `<input>` allows multiple files.
    pub fn is_multiple(&self) -> bool {
        matches!(self.event.mode, FileChooserOpenedEventMode::SelectMultiple)
    }

    /// Selection mode (single vs. multiple).
    pub fn mode(&self) -> FileChooserMode {
        self.event.mode.clone().into()
    }

    /// The raw CDP event for direct access to frame id etc.
    pub fn event(&self) -> &FileChooserOpenedEvent {
        &self.event
    }

    /// Supply the given paths as the user's file selection.
    ///
    /// Disables further file chooser interception once the selection has been
    /// delivered so subsequent choosers behave normally.
    pub async fn accept(&self, paths: Vec<PathBuf>) -> crate::Result<()> {
        let files: Vec<String> =
            paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let Some(backend_node_id) = self.event.backend_node_id else {
            // File choosers raised without a backing `<input>` can't be fed
            // files directly — caller should cancel instead.
            return Err(CdpError::NotFound);
        };
        let mut params = cdp_dom::SetFileInputFilesParams::new(files);
        params.backend_node_id = Some(backend_node_id);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Err(e) = self
            .handle
            .execute(
                cdp_page::SetInterceptFileChooserDialogParams::new(false),
                Some(self.session_id.current()),
            )
            .await
        {
            tracing::debug!(
                error = %e,
                "FileChooser::accept: failed to disable file-chooser interception after selection"
            );
        }
        Ok(())
    }

    /// Supply the given paths as the user's file selection.
    ///
    /// This is an alias for [`accept`](FileChooser::accept).
    pub async fn set_files(&self, paths: Vec<std::path::PathBuf>) -> crate::Result<()> {
        self.accept(paths).await
    }

    /// Returns the [`Element`](crate::Element) that triggered this file chooser, if available.
    pub async fn element(&self) -> crate::Result<Option<crate::element::Element>> {
        let backend_node_id = match self.event.backend_node_id {
            Some(id) => id,
            None => return Ok(None),
        };
        let desc_params = cdp_dom::DescribeNodeParams {
            backend_node_id: Some(backend_node_id),
            ..Default::default()
        };
        let desc = self.handle.execute(desc_params, Some(self.session_id.current())).await?;
        let node_id = desc.node.node_id;
        let elem = crate::element::Element::from_node_id(
            self.handle.clone(),
            self.session_id.clone(),
            node_id,
            None,
            None,
        )
        .await?;
        Ok(Some(elem))
    }

    /// Cancel the file chooser (no files selected).
    ///
    /// `setInterceptFileChooserDialog(enabled=true, cancel=true)` causes the
    /// next chooser to be cancelled; here we disable interception entirely
    /// which has the same practical effect for the already-open dialog while
    /// preserving clean state for subsequent choosers.
    pub async fn cancel(&self) -> crate::Result<()> {
        let mut params = cdp_page::SetInterceptFileChooserDialogParams::new(false);
        params.cancel = Some(true);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }
}
