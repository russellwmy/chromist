//! Per-page frame state: URL, loading status, and execution context IDs.
//!
//! [`FrameTree`] replaces the old `WorldsCache` (which only tracked execution
//! context IDs) with a richer model that also tracks the frame hierarchy and
//! loading state.  All mutations flow through [`FrameTree::apply_page_event`]
//! and [`FrameTree::apply_runtime_event`]; the background task in
//! [`Page::attach`](crate::page::Page::attach) calls both in response to the
//! session event stream.
//!
//! The root frame's ID is stored in `main_frame_id` and updated on every
//! `Page.frameNavigated` event without a `parent_id` — so `main_frame()` is
//! always a cheap map lookup, never a CDP round-trip.

use std::collections::HashMap;

use serde::Deserialize as _;
use serde_json::Value;

use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::js_protocol::runtime as cdp_runtime;

pub(crate) const UTILITY_WORLD_NAME: &str = "__chromist_utility_world__";

/// One frame's live state.
#[derive(Debug, Clone)]
pub(crate) struct FrameEntry {
    pub(crate) id: cdp_page::FrameId,
    pub(crate) parent_id: Option<cdp_page::FrameId>,
    pub(crate) url: String,
    pub(crate) name: Option<String>,
    /// `true` while `frameStartedLoading` has fired but `frameStoppedLoading`
    /// has not yet arrived.
    pub(crate) loading: bool,
    /// Main-world execution context for this frame.
    pub(crate) main_ctx: Option<cdp_runtime::ExecutionContextId>,
    /// `__chromist_utility_world__` execution context for this frame.
    pub(crate) utility_ctx: Option<cdp_runtime::ExecutionContextId>,
}

/// Per-page live frame registry.
///
/// Shared via `Arc<RwLock<FrameTree>>` between [`Page`](crate::page::Page)
/// and the background event task.
#[derive(Debug, Default)]
pub(crate) struct FrameTree {
    frames: HashMap<String, FrameEntry>,
    main_frame_id: Option<String>,
    /// Reverse lookup for O(1) context-destroyed handling:
    /// `context_id.0` → `(frame_id_str, is_utility)`.
    ctx_to_frame: HashMap<i64, (String, bool)>,
}

impl FrameTree {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    // ── Seeding from GetFrameTree ──────────────────────────────────────────

    /// Populate the tree from the response to `Page.GetFrameTree`.
    /// Called once in `Page::attach` before the background task starts.
    pub(crate) fn seed(&mut self, frame_tree: &cdp_page::FrameTree) {
        self.seed_recursive(frame_tree, None);
    }

    fn seed_recursive(
        &mut self,
        tree: &cdp_page::FrameTree,
        parent_id: Option<&cdp_page::FrameId>,
    ) {
        let f = &tree.frame;
        if parent_id.is_none() {
            self.main_frame_id = Some(f.id.inner().to_string());
        }
        self.frames.insert(
            f.id.inner().to_string(),
            FrameEntry {
                id: f.id.clone(),
                parent_id: f.parent_id.clone().or_else(|| parent_id.cloned()),
                url: f.url.clone(),
                name: f.name.clone(),
                loading: false,
                main_ctx: None,
                utility_ctx: None,
            },
        );
        if let Some(children) = &tree.child_frames {
            for child in children {
                self.seed_recursive(child, Some(&f.id));
            }
        }
    }

    // ── Page event handler ─────────────────────────────────────────────────

    /// Apply a raw `Page.*` event (frameAttached, frameNavigated, …).
    pub(crate) fn apply_page_event(&mut self, method: &str, params: &Value) {
        match method {
            "Page.frameAttached" => match cdp_page::FrameAttachedEvent::deserialize(params) {
                Err(e) => tracing::trace!(
                    method,
                    error = %e,
                    "dropping event: deserialization failed"
                ),
                Ok(ev) => {
                    self.frames.entry(ev.frame_id.inner().to_string()).or_insert_with(|| {
                        FrameEntry {
                            id: ev.frame_id,
                            parent_id: Some(ev.parent_frame_id),
                            url: String::new(),
                            name: None,
                            loading: false,
                            main_ctx: None,
                            utility_ctx: None,
                        }
                    });
                }
            },
            "Page.frameNavigated" => match cdp_page::FrameNavigatedEvent::deserialize(params) {
                Err(e) => tracing::trace!(
                    method,
                    error = %e,
                    "dropping event: deserialization failed"
                ),
                Ok(ev) => {
                    let f = &ev.frame;
                    let key = f.id.inner().to_string();
                    let is_root = f.parent_id.is_none();
                    if is_root {
                        self.main_frame_id = Some(key.clone());
                    }
                    let entry = self.frames.entry(key).or_insert_with(|| FrameEntry {
                        id: f.id.clone(),
                        parent_id: f.parent_id.clone(),
                        url: f.url.clone(),
                        name: f.name.clone(),
                        loading: false,
                        main_ctx: None,
                        utility_ctx: None,
                    });
                    // Navigation invalidates existing contexts for this frame.
                    if let Some(ctx) = entry.main_ctx.take() {
                        self.ctx_to_frame.remove(&ctx.0);
                    }
                    if let Some(ctx) = entry.utility_ctx.take() {
                        self.ctx_to_frame.remove(&ctx.0);
                    }
                    entry.url = f.url.clone();
                    entry.name = f.name.clone();
                    entry.parent_id = f.parent_id.clone();
                }
            },
            "Page.frameDetached" => match cdp_page::FrameDetachedEvent::deserialize(params) {
                Err(e) => tracing::trace!(
                    method,
                    error = %e,
                    "dropping event: deserialization failed"
                ),
                Ok(ev) => {
                    let key = ev.frame_id.inner();
                    if let Some(entry) = self.frames.remove(key) {
                        if let Some(ctx) = entry.main_ctx {
                            self.ctx_to_frame.remove(&ctx.0);
                        }
                        if let Some(ctx) = entry.utility_ctx {
                            self.ctx_to_frame.remove(&ctx.0);
                        }
                        if self.main_frame_id.as_deref() == Some(key) {
                            self.main_frame_id = None;
                        }
                    }
                }
            },
            "Page.frameStartedLoading" => {
                match cdp_page::FrameStartedLoadingEvent::deserialize(params) {
                    Err(e) => tracing::trace!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        if let Some(entry) = self.frames.get_mut(ev.frame_id.inner()) {
                            entry.loading = true;
                        }
                    }
                }
            }
            "Page.frameStoppedLoading" => {
                match cdp_page::FrameStoppedLoadingEvent::deserialize(params) {
                    Err(e) => tracing::trace!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        if let Some(entry) = self.frames.get_mut(ev.frame_id.inner()) {
                            entry.loading = false;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ── Runtime event handler ──────────────────────────────────────────────

    /// Apply a raw `Runtime.*` event (executionContextCreated/Destroyed/Cleared).
    pub(crate) fn apply_runtime_event(&mut self, method: &str, params: &Value) {
        match method {
            "Runtime.executionContextCreated" => {
                match cdp_runtime::ExecutionContextCreatedEvent::deserialize(params) {
                    Err(e) => tracing::trace!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        let ctx = &ev.context;
                        let frame_id = match ctx
                            .aux_data
                            .as_ref()
                            .and_then(|v| v.get("frameId"))
                            .and_then(|v| v.as_str())
                        {
                            Some(id) => id.to_string(),
                            None => return,
                        };
                        let is_default = ctx
                            .aux_data
                            .as_ref()
                            .and_then(|v| v.get("isDefault"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let is_utility = !is_default && ctx.name == UTILITY_WORLD_NAME;
                        if !is_default && !is_utility {
                            return;
                        }
                        let entry =
                            self.frames.entry(frame_id.clone()).or_insert_with(|| FrameEntry {
                                id: cdp_page::FrameId::new(&frame_id),
                                parent_id: None,
                                url: String::new(),
                                name: None,
                                loading: false,
                                main_ctx: None,
                                utility_ctx: None,
                            });
                        if is_default {
                            entry.main_ctx = Some(ctx.id);
                        } else {
                            entry.utility_ctx = Some(ctx.id);
                        }
                        self.ctx_to_frame.insert(ctx.id.0, (frame_id, is_utility));
                    }
                }
            }
            "Runtime.executionContextDestroyed" => {
                match cdp_runtime::ExecutionContextDestroyedEvent::deserialize(params) {
                    Err(e) => tracing::trace!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        if let Some((frame_id, is_utility)) =
                            self.ctx_to_frame.remove(&ev.execution_context_id.0)
                        {
                            if let Some(entry) = self.frames.get_mut(&frame_id) {
                                if is_utility {
                                    entry.utility_ctx = None;
                                } else {
                                    entry.main_ctx = None;
                                }
                            }
                        }
                    }
                }
            }
            "Runtime.executionContextsCleared" => {
                for entry in self.frames.values_mut() {
                    entry.main_ctx = None;
                    entry.utility_ctx = None;
                }
                self.ctx_to_frame.clear();
            }
            _ => {}
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────

    /// The root (main) frame, or `None` if the tree has not been seeded yet.
    pub(crate) fn main_frame(&self) -> Option<&FrameEntry> {
        self.frames.get(self.main_frame_id.as_deref()?)
    }

    /// Look up a frame by ID.
    pub(crate) fn get(&self, frame_id: &cdp_page::FrameId) -> Option<&FrameEntry> {
        self.frames.get(frame_id.inner())
    }

    /// Iterate over all known frames.
    pub(crate) fn all(&self) -> impl Iterator<Item = &FrameEntry> {
        self.frames.values()
    }

    /// The utility-world context for `frame_id`, if one has been created.
    pub(crate) fn utility_ctx_for_frame(
        &self,
        frame_id: &cdp_page::FrameId,
    ) -> Option<cdp_runtime::ExecutionContextId> {
        self.frames.get(frame_id.inner())?.utility_ctx
    }

    /// Return all immediate children of `parent_id`.
    pub(crate) fn children_of(&self, parent_id: &cdp_page::FrameId) -> Vec<&FrameEntry> {
        self.frames
            .values()
            .filter(|f| f.parent_id.as_ref().map(|p| p == parent_id).unwrap_or(false))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn navigated(id: &str, parent: Option<&str>, url: &str) -> Value {
        json!({
            "frame": {
                "id": id,
                "parentId": parent,
                "loaderId": "L1",
                "url": url,
                "domainAndRegistry": "",
                "securityOrigin": "",
                "mimeType": "text/html",
                "secureContextType": "Secure",
                "crossOriginIsolatedContextType": "Isolated",
                "gatedAPIFeatures": [],
            },
            "type": "Navigation",
        })
    }

    fn ctx_created(frame_id: &str, ctx_id: i64, is_default: bool, name: &str) -> Value {
        json!({
            "context": {
                "id": ctx_id,
                "origin": "",
                "name": name,
                "uniqueId": ctx_id.to_string(),
                "auxData": {
                    "frameId": frame_id,
                    "isDefault": is_default,
                    "type": if is_default { "default" } else { "isolated" },
                }
            }
        })
    }

    #[test]
    fn frame_navigated_sets_main_frame() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F-1", None, "https://a.com"));
        let main = tree.main_frame().expect("main frame set");
        assert_eq!(main.id.inner(), "F-1");
        assert_eq!(main.url, "https://a.com");
    }

    #[test]
    fn child_frame_attached_and_navigated() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("root", None, "https://a.com"));
        tree.apply_page_event(
            "Page.frameAttached",
            &json!({ "frameId": "child", "parentFrameId": "root" }),
        );
        tree.apply_page_event(
            "Page.frameNavigated",
            &navigated("child", Some("root"), "https://b.com"),
        );
        assert_eq!(tree.frames.len(), 2);
        let child = tree.get(&cdp_page::FrameId::new("child")).unwrap();
        assert_eq!(child.url, "https://b.com");
        assert_eq!(child.parent_id.as_ref().unwrap().inner(), "root");
    }

    #[test]
    fn frame_detached_removes_entry() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("root", None, "https://a.com"));
        tree.apply_page_event(
            "Page.frameAttached",
            &json!({ "frameId": "child", "parentFrameId": "root" }),
        );
        tree.apply_page_event(
            "Page.frameDetached",
            &json!({ "frameId": "child", "reason": "remove" }),
        );
        assert!(tree.get(&cdp_page::FrameId::new("child")).is_none());
        assert!(tree.main_frame().is_some()); // root unaffected
    }

    #[test]
    fn loading_flag_toggled() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://a.com"));
        tree.apply_page_event("Page.frameStartedLoading", &json!({ "frameId": "F" }));
        assert!(tree.main_frame().unwrap().loading);
        tree.apply_page_event("Page.frameStoppedLoading", &json!({ "frameId": "F" }));
        assert!(!tree.main_frame().unwrap().loading);
    }

    #[test]
    fn context_created_sets_main_and_utility_ctx() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://a.com"));
        tree.apply_runtime_event("Runtime.executionContextCreated", &ctx_created("F", 1, true, ""));
        tree.apply_runtime_event(
            "Runtime.executionContextCreated",
            &ctx_created("F", 2, false, UTILITY_WORLD_NAME),
        );
        let entry = tree.main_frame().unwrap();
        assert_eq!(entry.main_ctx.unwrap().0, 1);
        assert_eq!(entry.utility_ctx.unwrap().0, 2);
    }

    #[test]
    fn context_destroyed_clears_entry() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://a.com"));
        tree.apply_runtime_event("Runtime.executionContextCreated", &ctx_created("F", 1, true, ""));
        tree.apply_runtime_event(
            "Runtime.executionContextDestroyed",
            &json!({ "executionContextId": 1, "executionContextUniqueId": "1" }),
        );
        assert!(tree.main_frame().unwrap().main_ctx.is_none());
    }

    #[test]
    fn contexts_cleared_wipes_all_contexts() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://a.com"));
        tree.apply_runtime_event("Runtime.executionContextCreated", &ctx_created("F", 1, true, ""));
        tree.apply_runtime_event("Runtime.executionContextsCleared", &json!({}));
        assert!(tree.main_frame().unwrap().main_ctx.is_none());
        assert!(tree.ctx_to_frame.is_empty());
    }

    #[test]
    fn navigation_invalidates_old_contexts() {
        let mut tree = FrameTree::new();
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://a.com"));
        tree.apply_runtime_event("Runtime.executionContextCreated", &ctx_created("F", 1, true, ""));
        // Navigate again — contexts should be cleared from the entry.
        tree.apply_page_event("Page.frameNavigated", &navigated("F", None, "https://b.com"));
        assert!(tree.main_frame().unwrap().main_ctx.is_none());
        assert!(tree.ctx_to_frame.is_empty());
    }

    #[test]
    fn seed_populates_main_and_child_frames() {
        let mut tree = FrameTree::new();
        // Build a minimal FrameTree CDP response.
        use crate::cdp::browser_protocol::network::LoaderId;
        let cdp_tree = cdp_page::FrameTree {
            frame: cdp_page::Frame {
                id: cdp_page::FrameId::new("root"),
                parent_id: None,
                loader_id: LoaderId::new("L"),
                name: None,
                url: "https://a.com".into(),
                url_fragment: None,
                domain_and_registry: String::new(),
                security_origin: String::new(),
                security_origin_details: None,
                mime_type: "text/html".into(),
                unreachable_url: None,
                ad_frame_status: None,
                secure_context_type: cdp_page::SecureContextType::Secure,
                cross_origin_isolated_context_type:
                    cdp_page::CrossOriginIsolatedContextType::Isolated,
                gated_api_features: vec![],
            },
            child_frames: None,
        };
        tree.seed(&cdp_tree);
        assert_eq!(tree.main_frame().unwrap().id.inner(), "root");
    }
}
