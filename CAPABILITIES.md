# Capabilities

Reference index of every public capability in the `chromist` crate. **Use this document to find which API to call before grepping the source.** Each row gives the user-visible API and the source module that implements it; open the module with `Read` to see signatures and doc comments.

Conventions:
- All paths are relative to `crates/chromist/src/`.
- The API column shows the owning type (e.g. `Page::goto()`, `Locator::click()`).
- All listed APIs are public. For experimental hooks see [`page/raw_cdp.rs`](crates/chromist/src/page/raw_cdp.rs) and the `_bench` Cargo feature.

## Quick Index

- [Browser & Launch](#browser--launch)
- [Browser Context](#browser-context)
- [Navigation](#navigation)
- [DOM & Element Selection](#dom--element-selection)
- [Locator API](#locator-api)
- [Element API](#element-api)
- [JavaScript Evaluation](#javascript-evaluation)
- [Input Simulation](#input-simulation)
- [Screenshot & Capture](#screenshot--capture)
- [Cookie Management](#cookie-management)
- [Network Interception & Routing](#network-interception--routing)
- [Network Monitoring](#network-monitoring)
- [API Request Context](#api-request-context)
- [Frame API](#frame-api)
- [Device & Emulation](#device--emulation)
- [Clock Control (Fake Timers)](#clock-control-fake-timers)
- [Storage & Persistence](#storage--persistence)
- [Accessibility](#accessibility)
- [Page Events & Dialogs](#page-events--dialogs)
- [Downloads](#downloads)
- [File Chooser](#file-chooser)
- [WebSocket](#websocket)
- [Web Workers](#web-workers)
- [Function Exposure](#function-exposure)
- [Tracing & Coverage](#tracing--coverage)
- [Raw CDP Access](#raw-cdp-access)
- [Geometry & Layout Types](#geometry--layout-types)
- [Error Handling](#error-handling)
- [Cargo Feature Flags](#cargo-feature-flags)

---

## Browser & Launch

| API | Module |
|---|---|
| `Browser::launch()` | `browser/mod.rs` |
| `Browser::connect_to_http()`, `connect_over_cdp()` | `browser/mod.rs` |
| `Browser::launch_persistent_context()` | `browser/mod.rs` |
| `Browser::version()`, `user_agent()` | `browser/mod.rs` |
| `Browser::ws_url()` | `browser/mod.rs` |
| `Browser::targets()`, `get_page()` | `browser/mod.rs` |
| `Browser::new_page()` | `browser/mod.rs` |
| `Browser::create_browser_context()`, `dispose_browser_context()` | `browser/mod.rs` |
| `Browser::new_context_with_options()` | `browser/mod.rs` |
| `Browser::new_page_in_context()` | `browser/mod.rs` |
| `Browser::clear_cookies()`, `get_cookies()`, `set_cookies()` | `browser/mod.rs` |
| `Browser::new_target_stream()`, `target_destroyed_stream()`, `target_changed_stream()` | `browser/mod.rs` |
| `Browser::is_incognito()`, `is_connected()` | `browser/mod.rs` |
| `Browser::close()`, `kill()` | `browser/mod.rs` |
| `BrowserConfig::headless()` (new & legacy) | `browser/config.rs` |
| `BrowserConfig::use_pipe()` (Unix pipe transport) | `browser/config.rs` |
| `BrowserConfig::port()` (WebSocket transport) | `browser/config.rs` |
| `BrowserConfig::viewport()` | `browser/config.rs` |
| `BrowserConfig::user_data_dir()` | `browser/config.rs` |
| `BrowserConfig::extensions()` | `browser/config.rs` |
| `BrowserConfig::args()` (raw Chrome flags) | `browser/config.rs` |
| `BrowserConfig::sandbox()` | `browser/config.rs` |
| `BrowserConfig::incognito()` | `browser/config.rs` |
| `BrowserConfig::storage_state()` | `browser/config.rs` |
| `BrowserConfig::detection()` (executable auto-detect) | `detection.rs` |
| `BrowserConfig::process_envs()` | `browser/config.rs` |
| `BrowserConfig::proxy()`, `proxy_bypass_list()` | `browser/config.rs` |
| `BrowserConfig::slow_mo()` | `browser/config.rs` |
| `BrowserConfig::downloads_path()` | `browser/config.rs` |

---

## Browser Context

| API | Module |
|---|---|
| `BrowserContext::new_page()` | `context.rs` |
| `BrowserContext::pages()` | `context.rs` |
| `BrowserContext::close()` (disposes pages + context) | `context.rs` |
| `BrowserContext::wait_for_page()` | `context.rs` |
| `BrowserContext::wait_for_event<T>()` | `context.rs` |
| `BrowserContext::is_default()` | `handler/context.rs` |
| `BrowserContext::add_init_script()` | `handler/context.rs` |
| `BrowserContext::set_extra_http_headers()` | `handler/context.rs` |
| `BrowserContext::add_cookies()`, `clear_cookies()` | `handler/context.rs` |
| `BrowserContext::grant_permissions()`, `clear_permissions()` | `handler/context.rs` |
| `BrowserContext::set_offline()` | `context.rs` |
| `BrowserContext::set_geolocation()`, `clear_geolocation()` | `context.rs` |
| `BrowserContext::storage_state()` | `handler/context.rs` |
| `BrowserContext::route()` (URL-pattern interception) | `page/route.rs` |
| `BrowserContext::route_websocket()` | `handler/context.rs` |
| `BrowserContext::route_from_har()` (HAR playback) | `handler/context.rs`, `har.rs` |
| `BrowserContext::expose_function()`, `expose_binding()` | `context.rs` |
| `BrowserContext::new_cdp_session()` | `context.rs` |
| `BrowserContext::tracing()` | `tracing.rs` |
| `BrowserContextOptions::bypass_csp` | `context_options.rs` |
| `BrowserContextOptions::http_credentials`, `HttpCredentials` | `context_options.rs` |
| `BrowserContextOptions::javascript_enabled` | `context_options.rs` |
| `BrowserContextOptions::service_workers`, `ServiceWorkersPolicy` | `context_options.rs` |
| `BrowserContextOptions::downloads_path` | `context_options.rs` |
| `BrowserContext::service_workers()`, `wait_for_service_worker()` | `handler/context.rs`, `context.rs` |
| `BrowserContext::background_pages()`, `wait_for_background_page()` | `context.rs` |

---

## Navigation

| API | Module |
|---|---|
| `Page::goto()` | `page/navigation.rs` |
| `Page::goto_wait()`, `goto_with_wait()` | `page/navigation.rs` |
| `Page::reload()`, `reload_with_wait()` | `page/navigation.rs` |
| `Page::navigation_waiter()` (pre-armed) | `page/navigation.rs` |
| `WaitUntil::{Load, DOMContentLoaded, NetworkIdle, NetworkAlmostIdle}` | `lifecycle.rs` |
| `Page::url()`, `title()`, `opener()` | `page/mod.rs` |
| `Page::wait_for_request()`, `wait_for_response()` | `page/navigation.rs` |
| `Page::wait_for_navigation_response()` | `page/navigation.rs` |
| `Page::go_back()`, `go_forward()` | `page/navigation.rs` |
| `Page::bring_to_front()`, `activate()` | `page/navigation.rs` |
| `Page::wait_for_url()` | `page/navigation.rs` |
| `Page::wait_for_navigation_with()` (triggered) | `page/navigation.rs` |

---

## DOM & Element Selection

| API | Module |
|---|---|
| `Page::dom().find_element()`, `find_elements()` (CSS) | `page/dom.rs` |
| `Page::dom().find_xpath()`, `find_xpaths()` | `page/dom.rs` |
| `Page::locator()` (CSS locator) | `page/mod.rs` |
| `Page::get_by_role()` | `page/mod.rs` |
| `Page::get_by_text()` | `page/mod.rs` |
| `Page::get_by_label()` | `page/mod.rs` |
| `Page::get_by_placeholder()` | `page/mod.rs` |
| `Page::get_by_test_id()` | `page/mod.rs` |
| `Page::frame_locator()` | `page/mod.rs` |
| `Page::content()`, `set_content()` | `page/content.rs` |

---

## Locator API

| API | Module |
|---|---|
| `Locator::count()`, `all()` | `locator.rs` |
| `Locator::nth()`, `first()`, `last()` | `locator.rs` |
| `Locator::wait_for()` | `locator.rs` |
| `Locator::and()`, `or()` | `locator.rs` |
| `Locator::parent()`, `ancestor()` | `locator.rs` |
| `Locator::filter()` (JS predicate) | `locator.rs` |
| `Locator::non_strict()` | `locator.rs` |
| `Locator::get_by_*()` (semantic sub-locators) | `locator.rs` |
| `Locator::click()`, `dblclick()`, `tap()` | `locator.rs` |
| `Locator::fill()`, `type_()`, `clear()` | `locator.rs` |
| `Locator::check()`, `uncheck()` | `locator.rs` |
| `Locator::select_option()` | `locator.rs` |
| `Locator::hover()`, `focus()`, `blur()` | `locator.rs` |
| `Locator::scroll_into_view()` | `locator.rs` |
| `Locator::dispatch_event()` | `locator.rs` |
| `Locator::inner_text()`, `inner_html()` | `locator.rs` |
| `Locator::get_attribute()`, `attributes()` | `locator.rs` |
| `Locator::input_value()` | `locator.rs` |
| `Locator::evaluate()` | `locator.rs` |
| `Locator::is_visible()`, `is_hidden()`, etc. | `locator.rs` |
| `Locator::expect()` (assertion builder) | `locator.rs` |
| `Locator::content_frame()` (iframe) | `locator.rs` |
| `Locator::aria_snapshot()` | `locator.rs` |
| `Page::add_locator_handler()` | `page/mod.rs` |

---

## Element API

| API | Module |
|---|---|
| `Element::node_id()`, `backend_node_id()`, `object_id()` | `element.rs` |
| `Element::click()`, `dblclick()`, `tap()` | `element.rs` |
| `Element::fill()`, `type_()`, `clear()` | `element.rs` |
| `Element::check()`, `uncheck()` | `element.rs` |
| `Element::hover()`, `focus()`, `blur()` | `element.rs` |
| `Element::scroll_into_view()` | `element.rs` |
| `Element::dispatch_event()` | `element.rs` |
| `Element::inner_text()`, `inner_html()` | `element.rs` |
| `Element::get_attribute()`, `attributes()` | `element.rs` |
| `Element::input_value()` | `element.rs` |
| `Element::bounding_box()`, `box_model()` | `element.rs` |
| `Element::evaluate()` | `element.rs` |
| `Element::find_element()`, `find_elements()` (children) | `element.rs` |
| `Element::is_visible()`, `is_enabled()`, etc. | `element.rs` |

---

## JavaScript Evaluation

| API | Module |
|---|---|
| `Page::evaluate()` (expression / function) | `page/evaluation.rs` |
| `Page::evaluate_handle()` | `page/evaluation.rs` |
| `EvaluationResult::into_value()`, `is_truthy()` | `evaluate.rs` |
| `JsHandle::evaluate()`, `json_value()` | `js_handle.rs` |
| `JsHandle::as_element()` | `js_handle.rs` |
| `JsHandle::dispose()` | `js_handle.rs` |
| `IS_VISIBLE`, `IS_STABLE`, `IS_ENABLED`, `HIT_TEST` (built-in JS predicates) | `js.rs` |

---

## Input Simulation

| API | Module |
|---|---|
| `Mouse::move_to()`, `click()`, `dbl_click()` | `page/input.rs` |
| `Mouse::drag_to()`, `down()`, `up()`, `scroll()` | `page/input.rs` |
| `Keyboard::press()`, `type_()`, `down()`, `up()` | `page/input.rs` |
| `Keyboard` modifier tracking (Alt, Ctrl, Meta, Shift) | `page/input.rs` |
| `Touchscreen::tap()`, `long_tap()`, `swipe()` | `page/input.rs` |
| `USKEYBOARD_LAYOUT` | `keys.rs` |

---

## Screenshot & Capture

| API | Module |
|---|---|
| `Page::screenshot()` (full-page) | `page/capture.rs` |
| `Page::screenshot_bbox()` | `page/capture.rs` |
| `ScreenshotFormat::{Png, Jpeg, WebP}` | `page/capture.rs` |
| `ScreenshotParams` builder (quality, scale, clip) | `page/capture.rs` |

---

## Cookie Management

| API | Module |
|---|---|
| `Page::cookies()` | `page/cookies.rs` |
| `Page::set_cookie()`, `add_cookie()` | `page/cookies.rs` |
| `Page::delete_cookie()` | `page/cookies.rs` |

---

## Network Interception & Routing

| API | Module |
|---|---|
| `Page::route()` (URL pattern handler) | `page/route.rs` |
| `Page::route_once()` | `page/route.rs` |
| `Page::unroute()` | `page/route.rs` |
| `Route::abort()`, `continue_()`, `fulfill()` | `route.rs` |
| `RouteResponse` (synthetic response builder) | `route.rs` |
| `RouteRegistry` (glob URL matching) | `route.rs` |
| `Page::intercept_requests()` (raw Fetch) | `page/intercept.rs` |
| `Page::fulfill_request()` | `page/intercept.rs` |
| `Page::authenticate()` (HTTP Basic) | `page/intercept.rs` |
| `Page::route_websocket()` | `page/websocket_route.rs` |
| `BrowserContext::route_from_har()` (HAR playback) | `har.rs` |

---

## Network Monitoring

| API | Module |
|---|---|
| `NetworkManager::request_stream()`, `response_stream()`, `failed_stream()` | `network.rs` |
| `NetworkManager::get_request()`, `requests_snapshot()` | `network.rs` |
| `NetworkManager::response_body()`, `response_text()`, `response_json()` | `network.rs` |
| `HttpRequest::body()`, `text()`, `json()`, `finished()` | `network.rs` |
| `HttpRequest::{url, method, headers, post_data}` | `network.rs` |
| `HttpRequest::resource_type()`, `is_navigation_request()`, `frame_id()` | `network.rs` |
| `HttpRequest::redirect_chain` | `network.rs` |
| `HttpRequest::sizes()`, `RequestSizes` | `network.rs` |
| `HttpRequest::timing()` | `network.rs` |
| `HttpRequest::security_details()` (TLS) | `network.rs` |
| `HttpRequest::headers_array()`, `response_headers_array()` | `network.rs` |
| `HttpRequest::encoded_response_length` | `network.rs` |

---

## API Request Context

| API | Module |
|---|---|
| `APIRequestContext::new()` (standalone HTTP client) | `api_request.rs` |
| `APIRequestContext::from_page()` | `api_request.rs` |
| `APIRequestContext::{get, post, put, patch, delete}` | `api_request.rs` |
| `APIRequestContext::fetch()` | `api_request.rs` |
| `APIResponse` (bytes / text / JSON) | `api_request.rs` |

---

## Frame API

| API | Module |
|---|---|
| `Frame::url()`, `name()`, `is_loading()`, `parent_id()` | `frame.rs` |
| `Frame::find_element()`, `find_xpath()` | `frame.rs` |
| `Frame::evaluate()`, `evaluate_handle()` | `frame.rs` |
| `Frame::locator()`, `get_by_*()` | `frame.rs` |
| `FrameLocator::frame_locator()` (nested) | `frame_locator.rs` |
| `Frame::fill()`, `click()`, `check()`, `select_option()`, etc. | `frame.rs` |
| `Frame::get_attribute()`, `inner_text()`, `input_value()` | `frame.rs` |
| `Frame::is_visible()`, `is_hidden()`, `is_enabled()`, `is_disabled()`, `is_checked()` | `frame.rs` |
| `Frame::dispatch_event()` | `frame.rs` |

---

## Device & Emulation

| API | Module |
|---|---|
| `Page::emulate_device()` | `page/device.rs` |
| `Page::set_viewport()`, `set_user_agent()` | `page/emulation.rs` |
| `Page::emulate_geolocation()` | `page/emulation.rs` |
| `Page::emulate_media_type()`, `emulate_media_features()` | `page/emulation.rs` |
| `Page::emulate_timezone()`, `emulate_locale()` | `page/emulation.rs` |
| `Page::set_offline_mode()`, `set_cache_enabled()` | `page/emulation.rs` |
| `Page::set_extra_headers()` | `page/emulation.rs` |
| `Page::set_ignore_certificate_errors()`, `set_bypass_csp()` | `page/emulation.rs` |
| `Page::set_javascript_enabled()`, `set_download_behavior()` | `page/emulation.rs` |
| `Page::enable_stealth_mode()`, `enable_stealth_mode_full()`, `enable_stealth_mode_with_agent()` | `page/emulation.rs` |
| `Page::{enable,disable}_{log,runtime,dom,css,debugger,network}()` | `page/emulation.rs` |
| `DeviceDescriptor` (iPhone, Pixel, iPad, Galaxy) | `device.rs` |

---

## Clock Control (Fake Timers)

| API | Module |
|---|---|
| `Page::install_clock()` | `clock.rs` |
| `Clock::set_time()`, `tick()`, `restore()` | `clock.rs` |

---

## Storage & Persistence

| API | Module |
|---|---|
| `BrowserContext::storage_state()` (snapshot) | `storage.rs` |
| `BrowserConfig::storage_state()` (restore) | `storage.rs` |
| `StorageState::cookies` | `storage.rs` |
| `StorageState::origins` (localStorage) | `storage.rs` |

---

## Accessibility

| API | Module |
|---|---|
| `Page::accessibility_snapshot()` (full tree) | `page/accessibility.rs` |
| `Page::aria_snapshot()` (AI-friendly) | `page/accessibility.rs` |
| `Locator::aria_snapshot()` | `locator.rs` |
| `AXNode` (role, name, description, ignored, children) | `accessibility.rs` |

---

## Page Events & Dialogs

| API | Module |
|---|---|
| `Page::on_dialog()`, `dialog_stream()`, `dismiss_dialog()` | `page/events.rs` |
| `Dialog::accept()`, `type_()` | `dialog.rs` |
| `Page::on_console()` | `page/events.rs` |
| `ConsoleMessage::location`, `ConsoleLocation` | `events.rs` |
| `Page::on_page_error()` | `page/events.rs` |
| `Page::on_download()` | `page/events.rs` |
| `Page::on_worker()` | `page/events.rs` |
| `Page::on_websocket()` | `page/events.rs` |
| `Page::wait_for_file_chooser()` | `page/events.rs` |
| `Page::wait_for_event<T>()` (generic typed) | `page/events.rs` |
| `Page::wait_for_load_state()` | `page/events.rs` |

---

## Downloads

| API | Module |
|---|---|
| `Download::save_as()`, `delete()`, `cancel()` | `events.rs` |
| `Download::page()` (initiating page) | `events.rs` |
| `Download::create_read_stream()` | `events.rs` |

---

## File Chooser

| API | Module |
|---|---|
| `FileChooser::accept()`, `set_files()` | `file_chooser.rs` |
| `FileChooser::cancel()` | `file_chooser.rs` |
| `FileChooser::is_multiple()`, `mode()` | `file_chooser.rs` |
| `FileChooser::element()` | `file_chooser.rs` |

---

## WebSocket

| API | Module |
|---|---|
| `Page::on_websocket()` → `WebSocket` handle | `page/events.rs` |
| `WebSocket::is_closed()` | `websocket.rs` |
| `WebSocketEvent`, `WebSocketEventKind` | `websocket.rs` |

---

## Web Workers

| API | Module |
|---|---|
| `Page::on_worker()`, `Page::workers()` | `page/events.rs` |
| `Worker::evaluate()`, `evaluate_handle()` | `events.rs` |
| `Worker::on_console()`, `on_close()` | `events.rs` |

---

## Function Exposure

| API | Module |
|---|---|
| `Page::expose_function()` (Rust async fn → page JS) | `page/expose.rs` |
| `Page::expose_binding()` (with protocol access) | `page/expose.rs` |
| Promise return bridging (automatic) | `page/expose.rs` |
| `BrowserContext::expose_function()`, `expose_binding()` | `context.rs` |

---

## Tracing & Coverage

| API | Module |
|---|---|
| `Page::tracing()`, `BrowserContext::tracing()` | `tracing.rs` |
| `TracingSession::start()`, `stop()` | `tracing.rs` |
| `TracingSession::data_collected_stream()` | `tracing.rs` |
| `Page::js_coverage()`, `css_coverage()` | `coverage.rs` |
| `Page::metrics()` (performance) | `page/mod.rs` |

---

## Raw CDP Access

| API | Module |
|---|---|
| `Page::new_cdp_session()` | `cdp_session.rs` |
| `Page::session_id()` | `page/raw_cdp.rs` |
| `HandlerHandle` (command dispatch) | `handler/mod.rs` |
| `Page::context()` (back-reference) | `page/mod.rs` |
| `Page::set_default_timeout()` | `page/mod.rs` |

---

## Geometry & Layout Types

| Type | Module |
|---|---|
| `Point` (2D coordinate) | `layout.rs` |
| `Viewport` | `layout.rs` |
| `BoundingBox` | `layout.rs` |
| `BoxModel` | `layout.rs` |
| `ElementQuad` (four corners) | `layout.rs` |

---

## Error Handling

| Type | Module |
|---|---|
| `CdpError` (typed variants) | `error.rs` |
| `CdpError::LockPoisoned` | `error.rs` |
| `CdpError::JavascriptException` | `error.rs` |
| `CdpError::Timeout` | `error.rs` |
| `CdpError::NotFound` | `error.rs` |
| `CdpError::CannotCloseDefaultContext` | `error.rs` |

---

## Cargo Feature Flags

| Flag | Purpose | Reference |
|---|---|---|
| `integration-tests` | Gates real-browser tests under `crates/chromist/tests/`. Off by default; CI sets it on the `integration` job. | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| `_bench` | Internal — exposes hooks needed by Criterion benches. Not for public consumption. | [BENCHMARKS.md](BENCHMARKS.md) |
