//! [`Cookies`](crate::Cookies) sub-handle: read / write / delete cookies scoped to a page's
//! session.
//!
//! Obtained via [`Page::cookies`]. Cheaply cloneable.

use std::sync::Arc;

use super::Page;
use crate::cdp::browser_protocol::network as cdp_network;
use crate::handler::{HandlerHandle, SessionRef};

/// Sub-handle for cookie operations on a [`Page`]. Obtain via
/// [`Page::cookies`].
#[derive(Debug, Clone)]
pub struct Cookies {
    handle: HandlerHandle,
    session_id: SessionRef,
    /// Live page URL — used as the default scope for [`Cookies::set`](crate::Cookies::set).
    /// Captured at the time `Page::cookies()` is called via [`Page::url`].
    page_url: Option<String>,
}

impl Cookies {
    pub(in crate::page) fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        page_url: Option<String>,
    ) -> Self {
        Self { handle, session_id, page_url }
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    /// Returns all cookies visible to the current page.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn all(&self) -> crate::Result<Vec<crate::storage::Cookie>> {
        let resp =
            self.handle.execute(cdp_network::GetCookiesParams::default(), self.session()).await?;
        Ok(resp.cookies.into_iter().map(crate::storage::Cookie::from).collect())
    }

    /// Returns cookies stored for the given URLs, independent of the page's
    /// current navigation. Use this when you need a deterministic lookup
    /// against a specific origin — e.g. right after [`Cookies::set_many`](crate::Cookies::set_many)
    /// with a `url` or `domain` that differs from the live document.
    pub async fn for_urls(&self, urls: Vec<String>) -> crate::Result<Vec<crate::storage::Cookie>> {
        let params = cdp_network::GetCookiesParams { urls: Some(urls) };
        let resp = self.handle.execute(params, self.session()).await?;
        Ok(resp.cookies.into_iter().map(crate::storage::Cookie::from).collect())
    }

    /// Set a single cookie scoped to the current page's URL.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn set(
        &self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> crate::Result<()> {
        let url = self.page_url.clone().unwrap_or_default();
        let params = cdp_network::SetCookieParams {
            name: name.into(),
            value: value.into(),
            url: if url.is_empty() { None } else { Some(url) },
            domain: None,
            path: None,
            secure: None,
            http_only: None,
            same_site: None,
            expires: None,
            priority: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Set multiple cookies at once using a single CDP call.
    pub async fn set_many(&self, cookies: Vec<cdp_network::CookieParam>) -> crate::Result<()> {
        if cookies.is_empty() {
            return Ok(());
        }
        let params = cdp_network::SetCookiesParams::new(cookies);
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Delete a single cookie by name from this page's URL scope.
    pub async fn delete(&self, name: impl Into<String>) -> crate::Result<()> {
        self.handle
            .execute(cdp_network::DeleteCookiesParams::new(name.into()), self.session())
            .await?;
        Ok(())
    }

    /// Delete multiple cookies by name.
    pub async fn delete_many(&self, names: Vec<impl Into<String>>) -> crate::Result<()> {
        for name in names {
            self.handle
                .execute(cdp_network::DeleteCookiesParams::new(name.into()), self.session())
                .await?;
        }
        Ok(())
    }

    /// Delete cookies using full [`DeleteCookiesParams`](cdp_network::DeleteCookiesParams)
    /// records — needed for cookies scoped by domain or path rather than name alone.
    pub async fn delete_with_params(
        &self,
        params: Vec<cdp_network::DeleteCookiesParams>,
    ) -> crate::Result<()> {
        for p in params {
            self.handle.execute(p, self.session()).await?;
        }
        Ok(())
    }
}

impl Page {
    /// Returns the [`Cookies`](crate::Cookies) sub-handle for cookie reads and writes scoped
    /// to this page's session.
    ///
    /// The handle captures the page's current URL at construction time as
    /// the default scope for [`Cookies::set`](crate::Cookies::set); refresh by calling
    /// `page.cookies()` again after navigation if you need the new origin.
    pub fn cookies(&self) -> Cookies {
        Cookies::new(self.handle.clone(), self.session_id.clone(), self.url())
    }
}
