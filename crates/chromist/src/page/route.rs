//! `Page` methods for route interception.
//!
//! `Page::route` / `route_once` / `unroute` manage a [`RouteRegistry`] stored
//! in the `Page`. On the first registration a background task is spawned that
//! enables the `Fetch` domain and forwards every `Fetch.requestPaused` event
//! to the registry. When the registry becomes empty the task is aborted and
//! the `Fetch` domain is disabled.

use std::future::Future;
use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize as _;

use super::Page;
use crate::cdp::browser_protocol::fetch as cdp_fetch;
use crate::route::{Route, RouteHandler};

impl Page {
    /// Register a handler for all requests whose URL matches `pattern`.
    ///
    /// `pattern` is a glob where `*` matches within a path segment and `**`
    /// matches across segments. Rules are evaluated in registration order;
    /// the first match wins. Unmatched requests are automatically continued.
    ///
    /// The background `Fetch.requestPaused` listener is started on the first
    /// registration.
    #[tracing::instrument(skip(self, handler), fields(pattern), level = "debug")]
    pub fn route<F, Fut>(&self, pattern: impl Into<String>, handler: F)
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let pattern = pattern.into();
        tracing::Span::current().record("pattern", pattern.as_str());
        let handler: RouteHandler = Arc::new(move |r| Box::pin(handler(r)));
        let was_empty = self.route_registry.is_empty();
        self.route_registry.add(pattern, handler, false);
        if was_empty {
            tracing::debug!("starting Fetch.requestPaused listener");
            self.start_route_task();
        }
    }

    /// Register a one-shot handler: automatically removed after the first match.
    #[tracing::instrument(skip(self, handler), fields(pattern), level = "debug")]
    pub fn route_once<F, Fut>(&self, pattern: impl Into<String>, handler: F)
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let pattern = pattern.into();
        tracing::Span::current().record("pattern", pattern.as_str());
        let handler: RouteHandler = Arc::new(move |r| Box::pin(handler(r)));
        let was_empty = self.route_registry.is_empty();
        self.route_registry.add(pattern, handler, true);
        if was_empty {
            tracing::debug!("starting Fetch.requestPaused listener");
            self.start_route_task();
        }
    }

    /// Remove all handlers for `pattern`.
    ///
    /// When the registry becomes empty the background listener is stopped and
    /// the `Fetch` domain is disabled.
    pub async fn unroute(&self, pattern: &str) {
        self.route_registry.remove(pattern);
        if self.route_registry.is_empty() {
            self.stop_route_task().await;
        }
    }

    /// Spawn the background `Fetch.requestPaused` listener.
    ///
    /// This is a synchronous method — the actual `Fetch.enable` CDP call is
    /// issued from within the spawned task.
    pub(in crate::page) fn start_route_task(&self) {
        let handle = self.handle.clone();
        let session_id = self.session_id.clone();
        let registry = self.route_registry.clone();
        let context_registry = self.context_route_registry.clone();
        let task_ref = Arc::clone(&self.route_task);

        let task = crate::runtime::spawn(async move {
            let enable = cdp_fetch::EnableParams {
                patterns: Some(vec![cdp_fetch::RequestPattern {
                    url_pattern: Some("*".to_string()),
                    resource_type: None,
                    request_stage: None,
                }]),
                handle_auth_requests: Some(false),
            };
            if let Err(e) = handle.execute(enable, Some(session_id.current())).await {
                tracing::warn!(error = %e, "Fetch.enable failed; route handlers will not fire");
                return;
            }

            let mut events = handle.subscribe(Some(session_id.current()));
            while let Some(frame) = events.next().await {
                if frame.method != "Fetch.requestPaused" {
                    continue;
                }
                let event = match cdp_fetch::RequestPausedEvent::deserialize(&frame.params) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to deserialize Fetch.requestPaused event");
                        continue;
                    }
                };

                let route = Route {
                    event: event.clone(),
                    handle: handle.clone(),
                    session_id: session_id.clone(),
                };

                // Page-level registry takes priority over context-level.
                if registry.dispatch(route).await {
                    continue;
                }

                // Try context-level registry.
                if let Some(ref ctx_reg) = context_registry {
                    let ctx_route = Route {
                        event: event.clone(),
                        handle: handle.clone(),
                        session_id: session_id.clone(),
                    };
                    if ctx_reg.dispatch(ctx_route).await {
                        continue;
                    }
                }

                // No handler matched — auto-continue.
                tracing::trace!(url = %event.request.url, "no route handler matched; continuing request");
                let params = cdp_fetch::ContinueRequestParams::new(event.request_id);
                let _ = handle.execute(params, Some(session_id.current())).await;
            }
        });

        *task_ref.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::page::AbortOnDrop(task));
    }

    /// Abort the background listener task and disable the `Fetch` domain.
    pub(in crate::page) async fn stop_route_task(&self) {
        // Dropping the `AbortOnDrop` wrapper aborts the spawned task.
        drop(self.route_task.lock().unwrap_or_else(|e| e.into_inner()).take());
        let _ = self
            .handle
            .execute(cdp_fetch::DisableParams::default(), Some(self.session_id.current()))
            .await;
    }
}

// ---------------------------------------------------------------------------
// BrowserContext convenience re-export of RouteRegistry for context routing
// ---------------------------------------------------------------------------

impl crate::handler::BrowserContext {
    /// Register a context-wide route handler.
    ///
    /// Context-level rules are evaluated after page-level rules for any page
    /// that was created from this context and has its `context_route_registry`
    /// set.
    pub fn route<F, Fut>(&self, pattern: impl Into<String>, handler: F)
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handler: RouteHandler = Arc::new(move |r| Box::pin(handler(r)));
        self.registry.add(pattern.into(), handler, false);
    }
}
