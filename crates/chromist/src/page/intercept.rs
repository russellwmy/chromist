use futures::StreamExt;
use serde::Deserialize as _;

use super::Page;
use crate::cdp::browser_protocol::fetch as cdp_fetch;

impl Page {
    /// Enable request interception and authenticate with credentials.
    ///
    /// Spawns a background task that handles `Fetch.authRequired` events
    /// automatically. The task is owned by the page and aborted when the last
    /// `Page` clone is dropped.
    pub async fn authenticate(&self, credentials: crate::auth::Credentials) -> crate::Result<()> {
        let enable = cdp_fetch::EnableParams { patterns: None, handle_auth_requests: Some(true) };
        self.handle.execute(enable, Some(self.session_id.current())).await?;

        let handle = self.handle.clone();
        let session_id = self.session_id.clone();
        let creds = credentials;

        let join_handle = crate::runtime::spawn(async move {
            let mut attempted: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut events = handle.subscribe(Some(session_id.current()));
            while let Some(frame) = events.next().await {
                if frame.method == "Fetch.authRequired" {
                    match cdp_fetch::AuthRequiredEvent::deserialize(&frame.params) {
                        Ok(event) => {
                            let req_id_str = event.request_id.inner().to_string();
                            let response = if attempted.contains(&req_id_str) {
                                cdp_fetch::AuthChallengeResponseResponse::CancelAuth
                            } else {
                                attempted.insert(req_id_str);
                                cdp_fetch::AuthChallengeResponseResponse::ProvideCredentials
                            };
                            let mut auth = cdp_fetch::AuthChallengeResponse::new(response);
                            auth.username = Some(creds.username.clone());
                            auth.password = Some(creds.password.clone());
                            let params =
                                cdp_fetch::ContinueWithAuthParams::new(event.request_id, auth);
                            let _ = handle.execute(params, Some(session_id.current())).await;
                        }
                        Err(e) => tracing::debug!(
                            method = "Fetch.authRequired",
                            error = %e,
                            "dropping event: deserialization failed"
                        ),
                    }
                } else if frame.method == "Fetch.requestPaused" {
                    match cdp_fetch::RequestPausedEvent::deserialize(&frame.params) {
                        Ok(event) => {
                            let params = cdp_fetch::ContinueRequestParams::new(event.request_id);
                            let _ = handle.execute(params, Some(session_id.current())).await;
                        }
                        Err(e) => tracing::debug!(
                            method = "Fetch.requestPaused",
                            error = %e,
                            "dropping event: deserialization failed"
                        ),
                    }
                }
            }
        });
        self.register_listener_task(crate::AbortOnDrop(join_handle));
        Ok(())
    }

    /// Enable request interception. Returns an `EventStream` of `Fetch.requestPaused` events.
    pub async fn intercept_requests(
        &self,
    ) -> crate::Result<crate::listeners::EventStream<cdp_fetch::RequestPausedEvent>> {
        let enable = cdp_fetch::EnableParams {
            patterns: Some(vec![cdp_fetch::RequestPattern {
                url_pattern: Some("*".to_string()),
                resource_type: None,
                request_stage: None,
            }]),
            handle_auth_requests: Some(false),
        };
        self.handle.execute(enable, Some(self.session_id.current())).await?;
        Ok(self.handle.event_listener(Some(self.session_id.current())))
    }

    /// Fulfill a paused `Fetch.requestPaused` with a custom response.
    pub async fn fulfill_request(
        &self,
        params: cdp_fetch::FulfillRequestParams,
    ) -> crate::Result<()> {
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Continue a paused `Fetch.requestPaused` request, optionally modifying it.
    pub async fn continue_request(
        &self,
        params: cdp_fetch::ContinueRequestParams,
    ) -> crate::Result<()> {
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Abort a paused `Fetch.requestPaused` request with a network error.
    pub async fn abort_request(&self, params: cdp_fetch::FailRequestParams) -> crate::Result<()> {
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }
}
