//! HTTP Basic Auth credentials.

/// Username + password pair for HTTP Basic Auth challenges.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// HTTP Basic Auth username.
    pub username: String,
    /// HTTP Basic Auth password.
    pub password: String,
}

impl Credentials {
    /// Construct from username and password.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self { username: username.into(), password: password.into() }
    }
}
