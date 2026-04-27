//! [`Page::start_har_recording`] — convenience entry point for HAR recording.

use super::Page;
use crate::har_recorder::{HarRecorder, HarRecordingOptions};

impl Page {
    /// Start recording HTTP traffic for this page as a HAR 1.2 document.
    ///
    /// Network events are captured in a background task until
    /// [`HarRecorder::stop`] (or [`HarRecorder::content`] /
    /// [`HarRecorder::save_as`]) is called.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use chromist::{Browser, BrowserConfig};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let browser = Browser::launch(BrowserConfig::builder().build()).await?;
    /// # let page = browser.new_page("about:blank").await?;
    /// let recorder = page.start_har_recording(Default::default());
    /// page.goto("https://example.com").await?;
    /// let json = recorder.content()?;
    /// std::fs::write("out.har", &json)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn start_har_recording(&self, options: HarRecordingOptions) -> HarRecorder {
        HarRecorder::start(self.handle.clone(), self.session_id.clone(), options)
    }
}
