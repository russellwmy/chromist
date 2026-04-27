//! Per-target CDP session reference.
//!
//! Cross-process navigation in Chromium swaps the renderer for a page target;
//! the original CDP session is detached and a fresh one is attached with a
//! different `sessionId` (under `Target.setAutoAttach(autoAttach=true,
//! flatten=true)`). Anything that holds a frozen `Arc<str>` of the original
//! session id therefore breaks after the first cross-origin navigation: the
//! browser replies `-32001 Session with given id not found`.
//!
//! [`SessionRef`] is a cheaply cloneable handle that always resolves to the
//! *current* session id for a target. The handler owns the master cell per
//! target id and updates it from the I/O loop on
//! `Target.attachedToTarget` / `detachedFromTarget`.

use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct SessionRef(Arc<RwLock<Arc<str>>>);

impl SessionRef {
    pub(crate) fn new(id: Arc<str>) -> Self {
        Self(Arc::new(RwLock::new(id)))
    }

    /// Snapshot the current session id.
    pub fn current(&self) -> Arc<str> {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub(crate) fn store(&self, id: Arc<str>) {
        if let Ok(mut g) = self.0.write() {
            *g = id;
        }
    }
}

impl std::fmt::Debug for SessionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SessionRef").field(&self.current()).finish()
    }
}
