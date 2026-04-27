//! Event-driven registry of all known CDP targets and their attached sessions.
//!
//! [`TargetTree`] replaces the passive `target_cache` + `session_cells` +
//! `destroy_watchers` triad that previously lived as three separate
//! `Arc<RwLock/Mutex<…>>` fields on `Handler`.
//! All mutations are funnelled through [`TargetTree::apply_event`], which
//! the handler calls *before* fan-out to [`EventBus`](crate::event_bus::EventBus),
//! preserving the invariant that subscribers always observe up-to-date tree
//! state when they react to `Target.*` events.
//!
//! Parent/child relationships come from `TargetInfo.parent_id`, which Chrome
//! populates for `iframe` and `worker` targets, making workers and OOPIF
//! iframes addressable without any extra bookkeeping.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use serde::Deserialize as _;
use serde_json::Value;

use crate::cdp::browser_protocol::target as cdp_target;
use crate::handler::SessionRef;

/// One node in the target tree.
#[derive(Debug)]
pub(crate) struct TargetEntry {
    pub(crate) info: cdp_target::TargetInfo,
    /// Live session attached to this target, or `None` if not currently attached.
    pub(crate) session: Option<SessionRef>,
    /// Weak flags to flip when this target is destroyed.
    destroy_watchers: Vec<Weak<AtomicBool>>,
}

/// Event-driven registry of all CDP targets.
///
/// Holds every target Chrome has reported via `Target.targetCreated` or
/// `Target.attachedToTarget`, keyed by the raw target-ID string.
#[derive(Debug)]
pub(crate) struct TargetTree {
    entries: HashMap<String, TargetEntry>,
}

impl Default for TargetTree {
    fn default() -> Self {
        Self::new()
    }
}

impl TargetTree {
    pub(crate) fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    fn upsert_info(&mut self, info: cdp_target::TargetInfo) {
        let id = info.target_id.inner().clone();
        let entry = self.entries.entry(id).or_insert_with(|| TargetEntry {
            info: info.clone(),
            session: None,
            destroy_watchers: Vec::new(),
        });
        entry.info = info;
    }

    /// Apply a raw CDP `Target.*` event to the tree.
    ///
    /// Returns the target ID whose entry was modified (if any) so the caller
    /// can log or act on it without re-parsing the params.
    pub(crate) fn apply_event(&mut self, method: &str, params: &Value) {
        match method {
            "Target.targetCreated" => match cdp_target::TargetCreatedEvent::deserialize(params) {
                Err(e) => tracing::debug!(
                    method,
                    error = %e,
                    "dropping event: deserialization failed"
                ),
                Ok(ev) => self.upsert_info(ev.target_info),
            },
            "Target.targetInfoChanged" => {
                match cdp_target::TargetInfoChangedEvent::deserialize(params) {
                    Err(e) => tracing::debug!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => self.upsert_info(ev.target_info),
                }
            }
            "Target.targetDestroyed" => {
                match cdp_target::TargetDestroyedEvent::deserialize(params) {
                    Err(e) => tracing::debug!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        let key = ev.target_id.inner();
                        if let Some(entry) = self.entries.remove(key) {
                            for weak in entry.destroy_watchers {
                                if let Some(flag) = weak.upgrade() {
                                    flag.store(true, Ordering::Release);
                                }
                            }
                        }
                    }
                }
            }
            "Target.attachedToTarget" => {
                match cdp_target::AttachedToTargetEvent::deserialize(params) {
                    Err(e) => tracing::debug!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        let new_sid: Arc<str> = Arc::from(ev.session_id.0.as_str());
                        let id = ev.target_info.target_id.inner().clone();
                        let entry = self.entries.entry(id).or_insert_with(|| TargetEntry {
                            info: ev.target_info.clone(),
                            session: None,
                            destroy_watchers: Vec::new(),
                        });
                        entry.info = ev.target_info;
                        match &entry.session {
                            Some(cell) => cell.store(new_sid),
                            None => entry.session = Some(SessionRef::new(new_sid)),
                        }
                    }
                }
            }
            "Target.detachedFromTarget" => {
                match cdp_target::DetachedFromTargetEvent::deserialize(params) {
                    Err(e) => tracing::debug!(
                        method,
                        error = %e,
                        "dropping event: deserialization failed"
                    ),
                    Ok(ev) => {
                        // Match by session ID: scan all entries for the one whose
                        // session matches the detached session_id.
                        let detached_sid = ev.session_id.0.as_str();
                        for entry in self.entries.values_mut() {
                            if entry
                                .session
                                .as_ref()
                                .is_some_and(|s| s.current().as_ref() == detached_sid)
                            {
                                entry.session = None;
                                break;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Look up a target by ID.
    pub(crate) fn get(&self, target_id: &cdp_target::TargetId) -> Option<&TargetEntry> {
        self.entries.get(target_id.inner())
    }

    /// Iterate over all known targets.
    pub(crate) fn all(&self) -> impl Iterator<Item = &TargetEntry> {
        self.entries.values()
    }

    /// Iterate over direct children of `parent_id` (targets whose
    /// `TargetInfo.parent_id` matches).
    pub(crate) fn children_of(
        &self,
        parent_id: &cdp_target::TargetId,
    ) -> impl Iterator<Item = &TargetEntry> {
        let pid = parent_id.inner().clone();
        self.entries
            .values()
            .filter(move |e| e.info.parent_id.as_ref().is_some_and(|p| p.inner() == pid.as_str()))
    }

    /// Return the live `SessionRef` for a target, if it is currently attached.
    pub(crate) fn session_cell(&self, target_id: &cdp_target::TargetId) -> Option<SessionRef> {
        self.entries.get(target_id.inner())?.session.clone()
    }

    /// Return the existing `SessionRef` for a target, or insert one initialised
    /// to `initial`.  If the cell already exists, its stored ID is overwritten
    /// with `initial` (same renderer-swap behaviour as the previous handler).
    pub(crate) fn ensure_session(
        &mut self,
        target_id: &cdp_target::TargetId,
        initial: Arc<str>,
    ) -> SessionRef {
        let entry = self.entries.entry(target_id.inner().clone()).or_insert_with(|| TargetEntry {
            info: placeholder_info(target_id),
            session: None,
            destroy_watchers: Vec::new(),
        });
        match &entry.session {
            Some(cell) => {
                cell.store(initial);
                cell.clone()
            }
            None => {
                let cell = SessionRef::new(initial);
                entry.session = Some(cell.clone());
                cell
            }
        }
    }

    /// Register a destroy-watcher flag for a target. The flag is set to `true`
    /// when `Target.targetDestroyed` arrives for that target ID. Dead `Weak`s
    /// are pruned on the next destroy event for the same target.
    pub(crate) fn register_destroy_watcher(
        &mut self,
        target_id: &cdp_target::TargetId,
        flag: &Arc<AtomicBool>,
    ) {
        let entry = self.entries.entry(target_id.inner().clone()).or_insert_with(|| TargetEntry {
            info: placeholder_info(target_id),
            session: None,
            destroy_watchers: Vec::new(),
        });
        entry.destroy_watchers.retain(|w| w.strong_count() > 0);
        entry.destroy_watchers.push(Arc::downgrade(flag));
    }
}

/// Minimal `TargetInfo` used when an entry is created before `targetCreated`
/// fires (e.g. an `attachToTarget` reply beats the event).
fn placeholder_info(target_id: &cdp_target::TargetId) -> cdp_target::TargetInfo {
    cdp_target::TargetInfo {
        target_id: target_id.clone(),
        r#type: String::new(),
        title: String::new(),
        url: String::new(),
        attached: false,
        parent_id: None,
        opener_id: None,
        can_access_opener: false,
        opener_frame_id: None,
        parent_frame_id: None,
        browser_context_id: None,
        subtype: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target_created_params(id: &str) -> Value {
        json!({
            "targetInfo": {
                "targetId": id,
                "type": "page",
                "title": "",
                "url": "about:blank",
                "attached": false,
                "canAccessOpener": false,
            }
        })
    }

    fn attached_params(target_id: &str, session_id: &str) -> Value {
        json!({
            "sessionId": session_id,
            "targetInfo": {
                "targetId": target_id,
                "type": "page",
                "title": "",
                "url": "about:blank",
                "attached": true,
                "canAccessOpener": false,
            },
            "waitingForDebugger": false,
        })
    }

    fn tid(s: &str) -> cdp_target::TargetId {
        cdp_target::TargetId::from(s.to_string())
    }

    #[test]
    fn created_event_adds_entry() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.targetCreated", &target_created_params("T-1"));
        assert!(tree.get(&tid("T-1")).is_some());
    }

    #[test]
    fn destroyed_event_removes_entry() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.targetCreated", &target_created_params("T-1"));
        tree.apply_event("Target.targetDestroyed", &json!({ "targetId": "T-1" }));
        assert!(tree.get(&tid("T-1")).is_none());
    }

    #[test]
    fn destroyed_event_fires_watchers() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.targetCreated", &target_created_params("T-1"));
        let flag = Arc::new(AtomicBool::new(false));
        tree.register_destroy_watcher(&tid("T-1"), &flag);
        tree.apply_event("Target.targetDestroyed", &json!({ "targetId": "T-1" }));
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn attached_event_sets_session() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.attachedToTarget", &attached_params("T-1", "S-1"));
        let cell = tree.session_cell(&tid("T-1")).expect("session set");
        assert_eq!(&*cell.current(), "S-1");
    }

    #[test]
    fn attached_event_updates_session_on_renderer_swap() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.attachedToTarget", &attached_params("T-1", "S-1"));
        let cell = tree.session_cell(&tid("T-1")).unwrap();
        tree.apply_event("Target.attachedToTarget", &attached_params("T-1", "S-2"));
        assert_eq!(&*cell.current(), "S-2");
    }

    #[test]
    fn detached_event_clears_session() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.attachedToTarget", &attached_params("T-1", "S-1"));
        tree.apply_event(
            "Target.detachedFromTarget",
            &json!({ "sessionId": "S-1", "targetId": "T-1" }),
        );
        assert!(tree.get(&tid("T-1")).unwrap().session.is_none());
    }

    #[test]
    fn info_changed_updates_title() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.targetCreated", &target_created_params("T-1"));
        tree.apply_event(
            "Target.targetInfoChanged",
            &json!({
                "targetInfo": {
                    "targetId": "T-1",
                    "type": "page",
                    "title": "New Title",
                    "url": "https://example.com",
                    "attached": false,
                    "canAccessOpener": false,
                }
            }),
        );
        assert_eq!(tree.get(&tid("T-1")).unwrap().info.title, "New Title");
    }

    #[test]
    fn children_of_returns_correct_entries() {
        let mut tree = TargetTree::new();
        tree.apply_event("Target.targetCreated", &target_created_params("parent"));
        tree.apply_event(
            "Target.targetCreated",
            &json!({
                "targetInfo": {
                    "targetId": "child",
                    "type": "iframe",
                    "title": "",
                    "url": "about:blank",
                    "attached": false,
                    "canAccessOpener": false,
                    "parentId": "parent",
                }
            }),
        );
        tree.apply_event("Target.targetCreated", &target_created_params("other"));
        let children: Vec<_> = tree.children_of(&tid("parent")).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].info.target_id.inner().as_str(), "child");
    }

    #[test]
    fn ensure_session_creates_and_returns_same_ref() {
        let mut tree = TargetTree::new();
        let sid: Arc<str> = Arc::from("S-1");
        let cell = tree.ensure_session(&tid("T-1"), sid);
        assert_eq!(&*cell.current(), "S-1");
        // Calling again overwrites the session ID.
        let sid2: Arc<str> = Arc::from("S-2");
        let cell2 = tree.ensure_session(&tid("T-1"), sid2);
        assert_eq!(&*cell.current(), "S-2");
        assert_eq!(&*cell2.current(), "S-2");
    }
}
