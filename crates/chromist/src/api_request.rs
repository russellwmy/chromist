//! Thin HTTP client that optionally shares a page's browser cookies.
//!
//! [`APIRequestContext`] wraps a [`reqwest::Client`] and exposes ergonomic
//! `get`/`post`/`put`/`delete`/`fetch` methods that return [`APIResponse`].

use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::{Method, StatusCode};

use crate::error::CdpError;
use crate::page::Page;

/// Options for a single HTTP request.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct FetchOptions {
    /// HTTP method (default: `GET`).
    pub method: Option<String>,
    /// Request headers to merge with the context defaults.
    pub headers: HashMap<String, String>,
    /// JSON body — sets `Content-Type: application/json`.
    pub json: Option<serde_json::Value>,
    /// Raw request body bytes — set instead of `json` for non-JSON
    /// payloads. Mutually exclusive with `json`.
    pub body: Option<Vec<u8>>,
}

/// The response returned by [`APIRequestContext`] methods.
pub struct APIResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers as lowercase-key pairs.
    pub headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl APIResponse {
    /// Raw response body bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Decode the body as a UTF-8 string.
    pub fn text(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.clone())
    }

    /// Deserialize the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    /// Returns `true` when the status code indicates success (200–299).
    pub fn ok(&self) -> bool {
        StatusCode::from_u16(self.status).is_ok_and(|s| s.is_success())
    }
}

impl std::fmt::Debug for APIResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("APIResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// A thin HTTP client that optionally shares cookies with a browser page.
///
/// Build one with [`APIRequestContext::new`] for standalone requests, or with
/// [`APIRequestContext::from_page`] to inherit the page's current cookies.
///
/// ```no_run
/// # async fn example(page: chromist::Page) -> chromist::Result<()> {
/// use chromist::APIRequestContext;
/// let ctx = APIRequestContext::from_page(&page).await?;
/// let resp = ctx.get("https://api.example.com/data").await?;
/// let body: serde_json::Value = resp.json()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct APIRequestContext {
    client: Mutex<reqwest::Client>,
    base_url: Option<String>,
    default_headers: HashMap<String, String>,
}

impl APIRequestContext {
    /// Create a standalone request context with no shared cookies.
    pub fn new() -> crate::Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .map_err(|e| CdpError::RequestFailed(e.to_string()))?;
        Ok(Self { client: Mutex::new(client), base_url: None, default_headers: HashMap::new() })
    }

    /// Create a request context pre-seeded with the page's current cookies.
    pub async fn from_page(page: &Page) -> crate::Result<Self> {
        let ctx = Self::new()?;
        let cookies = page.cookies().all().await?;
        let jar = reqwest::cookie::Jar::default();
        for cookie in &cookies {
            let scheme = if cookie.secure { "https" } else { "http" };
            let domain = cookie.domain.trim_start_matches('.');
            let url_str = format!("{}://{}", scheme, domain);
            if let Ok(url) = url_str.parse() {
                let c = format!("{}={}", cookie.name, cookie.value);
                jar.add_cookie_str(&c, &url);
            }
        }
        // Rebuild client with the seeded jar.
        let client = reqwest::Client::builder()
            .cookie_provider(std::sync::Arc::new(jar))
            .build()
            .map_err(|e| CdpError::RequestFailed(e.to_string()))?;
        *ctx.client.lock().map_err(|_| CdpError::LockPoisoned)? = client;
        Ok(ctx)
    }

    /// Set a base URL that is prepended to relative paths.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Add a default header sent with every request.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.insert(key.into(), value.into());
        self
    }

    fn resolve_url(&self, url: &str) -> String {
        match &self.base_url {
            Some(base) if !url.starts_with("http://") && !url.starts_with("https://") => {
                format!("{}/{}", base.trim_end_matches('/'), url.trim_start_matches('/'))
            }
            _ => url.to_string(),
        }
    }

    /// Execute a request with full option control.
    pub async fn fetch(&self, url: &str, opts: FetchOptions) -> crate::Result<APIResponse> {
        let method_str = opts.method.as_deref().unwrap_or("GET").to_uppercase();
        let method = Method::from_bytes(method_str.as_bytes())
            .map_err(|_| CdpError::InvalidMethod(method_str))?;
        let resolved = self.resolve_url(url);

        let client = self.client.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        let mut builder = client.request(method, &resolved);

        for (k, v) in &self.default_headers {
            builder = builder.header(k, v);
        }
        for (k, v) in &opts.headers {
            builder = builder.header(k, v);
        }
        if let Some(json) = opts.json {
            builder = builder.json(&json);
        } else if let Some(body) = opts.body {
            builder = builder.body(body);
        }

        let resp = builder.send().await.map_err(|e| CdpError::RequestFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
            .collect();
        let body = resp.bytes().await.map_err(|e| CdpError::RequestFailed(e.to_string()))?.to_vec();

        Ok(APIResponse { status, headers, body })
    }

    /// GET `url`.
    pub async fn get(&self, url: &str) -> crate::Result<APIResponse> {
        self.fetch(url, FetchOptions { method: Some("GET".into()), ..Default::default() }).await
    }

    /// POST `url` with a JSON body.
    pub async fn post(&self, url: &str, json: serde_json::Value) -> crate::Result<APIResponse> {
        self.fetch(
            url,
            FetchOptions { method: Some("POST".into()), json: Some(json), ..Default::default() },
        )
        .await
    }

    /// PUT `url` with a JSON body.
    pub async fn put(&self, url: &str, json: serde_json::Value) -> crate::Result<APIResponse> {
        self.fetch(
            url,
            FetchOptions { method: Some("PUT".into()), json: Some(json), ..Default::default() },
        )
        .await
    }

    /// DELETE `url`.
    pub async fn delete(&self, url: &str) -> crate::Result<APIResponse> {
        self.fetch(url, FetchOptions { method: Some("DELETE".into()), ..Default::default() }).await
    }

    /// HEAD `url`.
    pub async fn head(&self, url: &str) -> crate::Result<APIResponse> {
        self.fetch(url, FetchOptions { method: Some("HEAD".into()), ..Default::default() }).await
    }

    /// PATCH `url` with a JSON body.
    pub async fn patch(&self, url: &str, json: serde_json::Value) -> crate::Result<APIResponse> {
        self.fetch(
            url,
            FetchOptions { method: Some("PATCH".into()), json: Some(json), ..Default::default() },
        )
        .await
    }
}
