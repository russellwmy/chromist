//! Integration tests — require a real Chromium on PATH.
//! Run with: cargo test -p chromist --test integration --features integration-tests
#![cfg(feature = "integration-tests")]

use chromist::{Browser, BrowserConfig};

#[tokio::test]
async fn evaluate_expression() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");
    let resp = page.evaluate("1 + 41").await.expect("evaluate");
    let val = resp.value().and_then(|v| v.as_u64());
    assert_eq!(val, Some(42));
}

#[tokio::test]
async fn screenshot_nonzero() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("https://example.com").await.expect("new_page");
    page.goto("https://example.com").await.expect("goto");
    let png = page.screenshot().await.expect("screenshot");
    assert!(!png.is_empty(), "screenshot must be non-empty");
}

#[tokio::test]
async fn find_element() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");
    page.set_content("<html><body><h1 id='t'>Hello</h1></body></html>").await.expect("set_content");
    let el = page.dom().find_element("h1#t").await.expect("find_element");
    let text = el.inner_text().await.expect("inner_text");
    assert_eq!(text, "Hello");
}

#[tokio::test]
async fn cookie_set_and_get() {
    use chromist::cdp::browser_protocol::network::CookieParam;

    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");

    // Hermetic: set the cookie with an explicit `url` so the cookie store
    // records it for that origin without requiring a live navigation, then
    // read it back with `cookies_for_urls` so the lookup doesn't depend on
    // the page's current document (important under CI with no external net).
    let origin = "https://example.com/";
    let mut cookie = CookieParam::new("test_key".to_string(), "test_value".to_string());
    cookie.url = Some(origin.to_string());
    page.cookies().set_many(vec![cookie]).await.expect("set_cookies");

    let cookies = page.cookies().for_urls(vec![origin.to_string()]).await.expect("cookies");
    let found = cookies.iter().any(|c| c.name == "test_key" && c.value == "test_value");
    assert!(found, "cookie 'test_key' not found in {:?}", cookies);
}

#[tokio::test]
async fn navigate_and_title() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");
    page.set_content("<html><head><title>My Test Title</title></head><body></body></html>")
        .await
        .expect("set_content");
    let title = page.title().await.expect("title");
    assert_eq!(title.as_deref(), Some("My Test Title"));
}

#[tokio::test]
async fn wait_for_selector() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");
    page.set_content("<html><body></body></html>").await.expect("set_content");
    // Add an element after a short delay via JS.
    page.evaluate(
        r#"setTimeout(() => {
            const el = document.createElement('div');
            el.id = 'late';
            el.textContent = 'appeared';
            document.body.appendChild(el);
        }, 200)"#,
    )
    .await
    .expect("evaluate");
    let el = page.dom().wait_for_selector("#late").await.expect("wait_for_selector");
    let text = el.inner_text().await.expect("inner_text");
    assert_eq!(text, "appeared");
}

#[tokio::test]
async fn evaluate_returns_error_on_exception() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");
    let page = browser.new_page("about:blank").await.expect("new_page");
    // eval() wraps a function literal in an IIFE; a thrown error produces
    // exception_details in the CDP response.
    let result = page.evaluate("() => { throw new Error('boom'); }").await.expect("evaluate");
    // The CDP response carries exception_details when JS throws; confirm it is present.
    assert!(
        result.into_inner().exception_details.is_some(),
        "expected exception_details to be set for a throwing function"
    );
}

#[tokio::test]
async fn new_page_and_close() {
    let config = BrowserConfig::builder().build();
    let browser = Browser::launch(config).await.expect("launch");

    let before = browser.targets().expect("targets before").len();
    let page = browser.new_page("about:blank").await.expect("new_page");
    let after_open = browser.targets().expect("targets after open").len();
    assert!(after_open > before, "target count should increase after new_page");

    page.close().await.expect("close");
    // Give the browser a moment to process the close.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let after_close = browser.targets().expect("targets after close").len();
    assert!(after_close < after_open, "target count should decrease after close");
}

#[cfg(unix)]
#[tokio::test]
async fn pipe_transport_evaluates_expression() {
    // Exercises the unsafe pipe-fd setup in `browser/mod.rs`. If the leak guard
    // or SAFETY invariants regress, this test is the canary.
    let config = BrowserConfig::builder().use_pipe().build();
    let browser = Browser::launch(config).await.expect("launch via pipe");
    let page = browser.new_page("about:blank").await.expect("new_page");
    let resp = page.evaluate("21 * 2").await.expect("evaluate");
    let val = resp.value().and_then(|v| v.as_u64());
    assert_eq!(val, Some(42));
}
