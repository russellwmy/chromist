use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize as _;

use super::Page;
use crate::cdp::browser_protocol::network as cdp_network;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::browser_protocol::target as cdp_target;
use crate::error::CdpError;

impl Page {
    /// Navigate to a URL and wait for the `load` lifecycle event.
    ///
    /// Equivalent to `goto_with(params, GotoOptions::default())`. For control
    /// over the wait condition, timeout, or `Referer` header, use
    /// [`goto_with`](Self::goto_with).
    pub async fn goto(
        &self,
        params: impl Into<cdp_page::NavigateParams>,
    ) -> crate::Result<crate::Navigation> {
        self.goto_with(params, crate::lifecycle::GotoOptions::default()).await
    }

    /// Navigate to a URL with explicit options.
    ///
    /// The lifecycle listener is armed *before* the navigate command is issued
    /// to avoid the race condition where the load fires before we subscribe.
    /// Use `GotoOptions::new().wait_until(WaitUntil::NoWait)` to skip waiting.
    #[tracing::instrument(skip(self, params, options), level = "debug")]
    pub async fn goto_with(
        &self,
        params: impl Into<cdp_page::NavigateParams>,
        options: crate::lifecycle::GotoOptions,
    ) -> crate::Result<crate::Navigation> {
        let waiter = self.navigation_waiter(options.wait_until).with_timeout(options.timeout);
        let mut params = params.into();
        if let Some(referer) = options.referer {
            params.referrer = Some(referer);
        }
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(err) = &resp.error_text {
            if !err.is_empty() {
                return Err(CdpError::NavigationFailed { reason: err.clone() });
            }
        }
        waiter.wait().await?;
        Ok(resp.into())
    }

    /// Create a pre-armed `NavigationWaiter` for the given condition.
    ///
    /// Subscribe *before* issuing the action that triggers navigation so that
    /// a fast load cannot fire before the subscription is registered.
    /// The waiter is scoped to the main frame so that subframe lifecycle events
    /// (OOPIF navigations) do not accidentally satisfy the condition.
    pub fn navigation_waiter(
        &self,
        condition: crate::lifecycle::WaitUntil,
    ) -> crate::lifecycle::NavigationWaiter {
        let frame_id =
            self.frame_tree.read().ok().and_then(|t| t.main_frame().map(|f| f.id.clone()));
        crate::lifecycle::NavigationWaiter {
            sub: self.handle.subscribe(Some(self.session_id.current())),
            condition,
            timeout: Duration::from_secs(30),
            frame_id,
        }
    }

    /// Returns a pre-armed waiter for use with click- or JS-triggered navigations.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let waiter = page.wait_for_navigation_with(chromist::WaitUntil::NetworkIdle);
    /// page.locator("a#nav").click().await?;
    /// waiter.wait().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn wait_for_navigation_with(
        &self,
        condition: crate::lifecycle::WaitUntil,
    ) -> crate::lifecycle::NavigationWaiter {
        self.navigation_waiter(condition)
    }

    /// Reload the current page and wait for the `load` lifecycle event.
    pub async fn reload(&self) -> crate::Result<()> {
        self.reload_with(crate::lifecycle::ReloadOptions::default()).await
    }

    /// Reload with explicit options.
    pub async fn reload_with(&self, options: crate::lifecycle::ReloadOptions) -> crate::Result<()> {
        let waiter = self.navigation_waiter(options.wait_until).with_timeout(options.timeout);
        self.handle
            .execute(cdp_page::ReloadParams::default(), Some(self.session_id.current()))
            .await?;
        waiter.wait().await
    }

    /// Bring this page's window to the foreground.
    pub async fn bring_to_front(&self) -> crate::Result<()> {
        self.handle
            .execute(cdp_page::BringToFrontParams::default(), Some(self.session_id.current()))
            .await?;
        Ok(())
    }

    /// Activate this page's target so it receives input events.
    pub async fn activate(&self) -> crate::Result<()> {
        self.handle
            .execute(cdp_target::ActivateTargetParams::new(self.target_id.clone()), None)
            .await?;
        Ok(())
    }

    /// Wait for the next load event (timeout 30 s).
    pub async fn wait_for_navigation(&self) -> crate::Result<()> {
        self.navigation_waiter(crate::lifecycle::WaitUntil::Load).wait().await
    }

    /// Wait for a specific lifecycle event (e.g. "load", "DOMContentLoaded", "networkIdle").
    pub async fn wait_for_lifecycle_event(&self, event_name: &str) -> crate::Result<()> {
        let name = event_name.to_string();
        let mut stream = self.handle.subscribe(Some(self.session_id.current()));
        crate::runtime::timeout(Duration::from_secs(30), async move {
            while let Some(frame) = stream.next().await {
                if frame.method == "Page.lifecycleEvent" {
                    if let Some(n) = frame.params.get("name").and_then(|v| v.as_str()) {
                        if n == name {
                            return;
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?;
        Ok(())
    }

    /// Navigate to the previous entry in the browser history.
    ///
    /// Returns `Ok(true)` when a back-navigation was performed, `Ok(false)` when
    /// there is no previous entry.
    pub async fn go_back(&self) -> crate::Result<bool> {
        let resp = self
            .handle
            .execute(
                cdp_page::GetNavigationHistoryParams::default(),
                Some(self.session_id.current()),
            )
            .await?;
        let idx = resp.current_index as usize;
        if idx == 0 {
            return Ok(false);
        }
        let entry = &resp.entries[idx - 1];
        let waiter = self.navigation_waiter(crate::lifecycle::WaitUntil::Load);
        self.handle
            .execute(
                cdp_page::NavigateToHistoryEntryParams::new(entry.id),
                Some(self.session_id.current()),
            )
            .await?;
        waiter.wait().await?;
        Ok(true)
    }

    /// Navigate to the next entry in the browser history.
    ///
    /// Returns `Ok(true)` when a forward-navigation was performed, `Ok(false)` when
    /// there is no next entry.
    pub async fn go_forward(&self) -> crate::Result<bool> {
        let resp = self
            .handle
            .execute(
                cdp_page::GetNavigationHistoryParams::default(),
                Some(self.session_id.current()),
            )
            .await?;
        let idx = resp.current_index as usize;
        if idx + 1 >= resp.entries.len() {
            return Ok(false);
        }
        let entry = &resp.entries[idx + 1];
        let waiter = self.navigation_waiter(crate::lifecycle::WaitUntil::Load);
        self.handle
            .execute(
                cdp_page::NavigateToHistoryEntryParams::new(entry.id),
                Some(self.session_id.current()),
            )
            .await?;
        waiter.wait().await?;
        Ok(true)
    }

    /// Wait until the page URL contains `pattern` (30 s timeout).
    pub async fn wait_for_url(&self, pattern: impl Into<String>) -> crate::Result<()> {
        let pattern = pattern.into();
        let frame_tree = std::sync::Arc::clone(&self.frame_tree);
        crate::runtime::timeout(Duration::from_secs(30), async move {
            loop {
                let url = frame_tree
                    .read()
                    .ok()
                    .and_then(|t| t.main_frame().map(|f| f.url.clone()))
                    .unwrap_or_default();
                if url.contains(pattern.as_str()) {
                    return Ok(());
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Wait for a `Network.requestWillBeSent` event whose URL satisfies `predicate`.
    ///
    /// Enables the `Network` domain automatically. 30 s timeout.
    pub async fn wait_for_request(
        &self,
        predicate: impl Fn(&str) -> bool + Send + 'static,
    ) -> crate::Result<Arc<crate::network::HttpRequest>> {
        let _ = self
            .handle
            .execute(
                crate::cdp::browser_protocol::network::EnableParams::default(),
                Some(self.session_id.current()),
            )
            .await;
        let mut sub = self.handle.subscribe(Some(self.session_id.current()));
        crate::runtime::timeout(Duration::from_secs(30), async move {
            while let Some(frame) = sub.next().await {
                if frame.method == "Network.requestWillBeSent" {
                    match cdp_network::RequestWillBeSentEvent::deserialize(&frame.params) {
                        Err(e) => tracing::trace!(
                            method = "Network.requestWillBeSent",
                            error = %e,
                            "dropping event: deserialization failed"
                        ),
                        Ok(ev) => {
                            if predicate(&ev.request.url) {
                                return Ok(Arc::new(crate::network::HttpRequest {
                                    request_id: ev.request_id,
                                    url: ev.request.url.clone(),
                                    method: ev.request.method.clone(),
                                    headers: std::collections::HashMap::new(),
                                    post_data: ev.request.post_data.clone(),
                                    frame_id: ev.frame_id,
                                    response: None,
                                    failure_text: None,
                                    from_memory_cache: false,
                                    interception_id: None,
                                    is_navigation_request: false,
                                    allow_interception: false,
                                    resource_type: ev.r#type,
                                    redirect_chain: Vec::new(),
                                    timestamp: Some(*ev.timestamp.inner()),
                                    encoded_response_length: None,
                                    handle: None,
                                    session_id: None,
                                }));
                            }
                        }
                    }
                }
            }
            Err(CdpError::ChannelClosed)
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Wait for a `Network.responseReceived` event whose URL satisfies `predicate`.
    ///
    /// Enables the `Network` domain automatically. 30 s timeout.
    pub async fn wait_for_response(
        &self,
        predicate: impl Fn(&str) -> bool + Send + 'static,
    ) -> crate::Result<Arc<crate::network::HttpRequest>> {
        let _ = self
            .handle
            .execute(
                crate::cdp::browser_protocol::network::EnableParams::default(),
                Some(self.session_id.current()),
            )
            .await;
        let mut sub = self.handle.subscribe(Some(self.session_id.current()));
        crate::runtime::timeout(Duration::from_secs(30), async move {
            while let Some(frame) = sub.next().await {
                if frame.method == "Network.responseReceived" {
                    match cdp_network::ResponseReceivedEvent::deserialize(&frame.params) {
                        Err(e) => tracing::trace!(
                            method = "Network.responseReceived",
                            error = %e,
                            "dropping event: deserialization failed"
                        ),
                        Ok(ev) => {
                            if predicate(&ev.response.url) {
                                return Ok(Arc::new(crate::network::HttpRequest {
                                    request_id: ev.request_id,
                                    url: ev.response.url.clone(),
                                    method: String::new(),
                                    headers: std::collections::HashMap::new(),
                                    post_data: None,
                                    frame_id: ev.frame_id,
                                    response: Some(ev.response),
                                    failure_text: None,
                                    from_memory_cache: false,
                                    interception_id: None,
                                    is_navigation_request: false,
                                    allow_interception: false,
                                    resource_type: Some(ev.r#type),
                                    redirect_chain: Vec::new(),
                                    timestamp: None,
                                    encoded_response_length: None,
                                    handle: None,
                                    session_id: None,
                                }));
                            }
                        }
                    }
                }
            }
            Err(CdpError::ChannelClosed)
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Wait for the next `Network.responseReceived` event and return the captured request.
    pub async fn wait_for_navigation_response(
        &self,
    ) -> crate::Result<crate::network::ArcHttpRequest> {
        let _ = self
            .handle
            .execute(cdp_network::EnableParams::default(), Some(self.session_id.current()))
            .await;
        let mut sub = self.handle.subscribe(Some(self.session_id.current()));
        let mut http_req: crate::network::ArcHttpRequest = None;
        crate::runtime::timeout(Duration::from_secs(30), async {
            while let Some(frame) = sub.next().await {
                if frame.method == "Network.responseReceived" {
                    match cdp_network::ResponseReceivedEvent::deserialize(&frame.params) {
                        Err(e) => tracing::trace!(
                            method = "Network.responseReceived",
                            error = %e,
                            "dropping event: deserialization failed"
                        ),
                        Ok(ev) => {
                            http_req = Some(Arc::new(crate::network::HttpRequest {
                                request_id: ev.request_id,
                                url: ev.response.url.clone(),
                                method: String::new(),
                                headers: std::collections::HashMap::new(),
                                post_data: None,
                                frame_id: ev.frame_id,
                                response: Some(ev.response),
                                failure_text: None,
                                from_memory_cache: false,
                                interception_id: None,
                                is_navigation_request: false,
                                allow_interception: false,
                                resource_type: None,
                                redirect_chain: Vec::new(),
                                timestamp: None,
                                encoded_response_length: None,
                                handle: None,
                                session_id: None,
                            }));
                        }
                    }
                } else if frame.method == "Page.loadEventFired" {
                    break;
                }
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?;
        Ok(http_req)
    }
}
