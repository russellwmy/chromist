use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use super::Page;
use crate::cdp::browser_protocol::input as cdp_input;
use crate::handler::{HandlerHandle, SessionRef};
use crate::layout::Point;

/// Which mouse button to use for synthesised pointer events.
#[non_exhaustive]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    /// Primary mouse button.
    Left,
    /// Secondary (context-menu) mouse button.
    Right,
    /// Middle (wheel) mouse button.
    Middle,
    #[default]
    /// No button — used for synthesised pointer-move events.
    None,
}

/// Options for synthesised click events.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ClickOptions {
    /// Number of consecutive clicks (1 = single, 2 = double, 3 = triple).
    pub click_count: u32,
    /// Which mouse button to fire.
    pub button: MouseButton,
}

// ── CDP modifier bitmask values ───────────────────────────────────────────────
const MOD_ALT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_META: u32 = 4;
const MOD_SHIFT: u32 = 8;

fn modifier_bit(key: &str) -> u32 {
    match key {
        "Alt" => MOD_ALT,
        "Control" => MOD_CTRL,
        "Meta" => MOD_META,
        "Shift" => MOD_SHIFT,
        _ => 0,
    }
}

// ── Mouse ─────────────────────────────────────────────────────────────────────

/// Stateful mouse handle that tracks the cursor position across calls.
///
/// Obtained via [`Page::mouse`]. All methods fire CDP `Input.dispatchMouseEvent`
/// commands using the session associated with the page.
///
/// Cheaply cloneable — all fields are `Arc`-wrapped.
#[derive(Debug, Clone)]
pub struct Mouse {
    pub(in crate::page) handle: HandlerHandle,
    pub(in crate::page) session_id: SessionRef,
    position: Arc<(AtomicU32, AtomicU32)>, // x/y stored as IEEE-754 bits
}

impl Mouse {
    pub(in crate::page) fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id, position: Arc::new((AtomicU32::new(0), AtomicU32::new(0))) }
    }

    fn read_position(&self) -> Point {
        let x = f32::from_bits(self.position.0.load(Ordering::Relaxed)) as f64;
        let y = f32::from_bits(self.position.1.load(Ordering::Relaxed)) as f64;
        Point { x, y }
    }

    fn write_position(&self, p: Point) {
        self.position.0.store((p.x as f32).to_bits(), Ordering::Relaxed);
        self.position.1.store((p.y as f32).to_bits(), Ordering::Relaxed);
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    /// Move the cursor to `(x, y)` and fire a `mousemoved` event.
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn move_to(&self, x: f64, y: f64) -> crate::Result<()> {
        let p = Point { x, y };
        let moved = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseMoved,
            p.x,
            p.y,
        );
        self.handle.execute(moved, self.session()).await?;
        self.write_position(p);
        Ok(())
    }

    /// Press the given button at the current cursor position.
    pub async fn down(&self, button: MouseButton) -> crate::Result<()> {
        self.down_with_count(button, 1).await
    }

    async fn down_with_count(&self, button: MouseButton, count: i64) -> crate::Result<()> {
        let pos = self.read_position();
        let mut ev = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MousePressed,
            pos.x,
            pos.y,
        );
        ev.button = Some(mouse_button_to_cdp(&button));
        ev.click_count = Some(count);
        self.handle.execute(ev, self.session()).await?;
        Ok(())
    }

    /// Release the given button at the current cursor position.
    pub async fn up(&self, button: MouseButton) -> crate::Result<()> {
        self.up_with_count(button, 1).await
    }

    async fn up_with_count(&self, button: MouseButton, count: i64) -> crate::Result<()> {
        let pos = self.read_position();
        let mut ev = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseReleased,
            pos.x,
            pos.y,
        );
        ev.button = Some(mouse_button_to_cdp(&button));
        ev.click_count = Some(count);
        self.handle.execute(ev, self.session()).await?;
        Ok(())
    }

    /// Move to `(x, y)`, press and release the left button (single click).
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn click(&self, x: f64, y: f64) -> crate::Result<()> {
        self.move_to(x, y).await?;
        self.down(MouseButton::Left).await?;
        self.up(MouseButton::Left).await
    }

    /// Move to `(x, y)`, press and release the left button twice (double-click).
    pub async fn dblclick(&self, x: f64, y: f64) -> crate::Result<()> {
        self.move_to(x, y).await?;
        self.down_with_count(MouseButton::Left, 2).await?;
        self.up_with_count(MouseButton::Left, 2).await
    }

    /// Press the left button at the current position, move to `target`, then release.
    pub async fn drag_to(&self, target: Point) -> crate::Result<()> {
        self.down(MouseButton::Left).await?;
        self.move_to(target.x, target.y).await?;
        self.up(MouseButton::Left).await
    }

    /// Scroll at the current cursor position by `(delta_x, delta_y)` pixels.
    ///
    /// Uses `Input.dispatchMouseEvent` with type `mouseWheel`.
    pub async fn wheel(&self, delta_x: f64, delta_y: f64) -> crate::Result<()> {
        let pos = self.read_position();
        let mut ev = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseWheel,
            pos.x,
            pos.y,
        );
        ev.delta_x = Some(delta_x);
        ev.delta_y = Some(delta_y);
        self.handle.execute(ev, self.session()).await?;
        Ok(())
    }
}

fn mouse_button_to_cdp(b: &MouseButton) -> cdp_input::MouseButton {
    match b {
        MouseButton::Left => cdp_input::MouseButton::Left,
        MouseButton::Right => cdp_input::MouseButton::Right,
        MouseButton::Middle => cdp_input::MouseButton::Middle,
        MouseButton::None => cdp_input::MouseButton::None,
    }
}

// ── Keyboard ──────────────────────────────────────────────────────────────────

/// Stateful keyboard handle that tracks currently-held modifier keys.
///
/// Obtained via [`Page::keyboard`]. Uses CDP `Input.dispatchKeyEvent` for
/// individual key events and `Input.insertText` for text input.
///
/// Cheaply cloneable — all fields are `Arc`-wrapped.
#[derive(Debug, Clone)]
pub struct Keyboard {
    pub(in crate::page) handle: HandlerHandle,
    pub(in crate::page) session_id: SessionRef,
    modifiers: Arc<AtomicU32>,
}

impl Keyboard {
    pub(in crate::page) fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id, modifiers: Arc::new(AtomicU32::new(0)) }
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    fn current_modifiers(&self) -> i64 {
        self.modifiers.load(Ordering::Relaxed) as i64
    }

    /// Press a key (fire `keyDown`). Updates the modifier bitmask for modifier keys.
    pub async fn down(&self, key: &str) -> crate::Result<()> {
        let bit = modifier_bit(key);
        if bit != 0 {
            self.modifiers.fetch_or(bit, Ordering::Relaxed);
        }
        let modifiers = self.current_modifiers();
        let mut ev =
            cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::KeyDown);
        apply_key_definition(&mut ev, key);
        ev.modifiers = Some(modifiers);
        self.handle.execute(ev, self.session()).await?;
        Ok(())
    }

    /// Release a key (fire `keyUp`). Clears the modifier bitmask for modifier keys.
    pub async fn up(&self, key: &str) -> crate::Result<()> {
        let bit = modifier_bit(key);
        if bit != 0 {
            self.modifiers.fetch_and(!bit, Ordering::Relaxed);
        }
        let modifiers = self.current_modifiers();
        let mut ev =
            cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::KeyUp);
        apply_key_definition(&mut ev, key);
        ev.modifiers = Some(modifiers);
        self.handle.execute(ev, self.session()).await?;
        Ok(())
    }

    /// Press and release a key (fires `keyDown` then `keyUp`).
    pub async fn press(&self, key: &str) -> crate::Result<()> {
        self.down(key).await?;
        self.up(key).await
    }

    /// Insert text directly without synthesising individual key events.
    ///
    /// This is the most reliable way to type arbitrary Unicode text. It uses
    /// `Input.insertText` which bypasses keymap translation entirely.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn type_text(&self, text: &str) -> crate::Result<()> {
        let params = cdp_input::InsertTextParams::new(text.to_string());
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Alias for [`type_text`](Keyboard::type_text) — inserts text via `Input.insertText`.
    pub async fn insert_text(&self, text: &str) -> crate::Result<()> {
        self.type_text(text).await
    }
}

// ── Touchscreen ───────────────────────────────────────────────────────────────

/// Touchscreen handle that synthesises touch events on the page.
///
/// Obtained via [`Page::touchscreen`]. Uses `Input.dispatchTouchEvent`.
///
/// Cheaply cloneable — holds only shared references.
#[derive(Debug, Clone)]
pub struct Touchscreen {
    pub(in crate::page) handle: HandlerHandle,
    pub(in crate::page) session_id: SessionRef,
}

impl Touchscreen {
    pub(in crate::page) fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id }
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    /// Synthesise a touch-tap at `(x, y)` — fires `touchstart` then `touchend`.
    pub async fn tap(&self, x: f64, y: f64) -> crate::Result<()> {
        let touch_point = cdp_input::TouchPoint::new(x, y);
        let start = cdp_input::DispatchTouchEventParams::new(
            cdp_input::DispatchTouchEventParamsType::TouchStart,
            vec![touch_point],
        );
        self.handle.execute(start, self.session()).await?;
        let end = cdp_input::DispatchTouchEventParams::new(
            cdp_input::DispatchTouchEventParamsType::TouchEnd,
            vec![],
        );
        self.handle.execute(end, self.session()).await?;
        Ok(())
    }
}

fn apply_key_definition(ev: &mut cdp_input::DispatchKeyEventParams, key: &str) {
    if let Some(def) = crate::keys::key_definition(key) {
        ev.key = Some(def.key.to_string());
        ev.code = Some(def.code.to_string());
        ev.windows_virtual_key_code = Some(def.key_code);
        ev.native_virtual_key_code = Some(def.key_code);
        if let Some(text) = def.text {
            ev.text = Some(text.to_string());
            ev.unmodified_text = Some(text.to_string());
        }
    } else {
        // Fallback: treat the string itself as the key value.
        ev.key = Some(key.to_string());
        if key.chars().count() == 1 {
            ev.text = Some(key.to_string());
            ev.unmodified_text = Some(key.to_string());
        }
    }
}

// ── Page dispatch methods (legacy stateless surface kept intact) ──────────────

impl Page {
    /// Move the mouse to a point.
    pub async fn move_mouse(&self, point: crate::layout::Point) -> crate::Result<()> {
        let session = Some(self.session_id.current());
        let moved = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseMoved,
            point.x,
            point.y,
        );
        self.handle.execute(moved, session).await?;
        Ok(())
    }

    /// Click at a specific point on the page.
    pub async fn click(&self, point: crate::layout::Point) -> crate::Result<()> {
        let session = Some(self.session_id.current());
        let mut down = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MousePressed,
            point.x,
            point.y,
        );
        down.button = Some(cdp_input::MouseButton::Left);
        down.click_count = Some(1);
        self.handle.execute(down, session.clone()).await?;
        let mut up = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseReleased,
            point.x,
            point.y,
        );
        up.button = Some(cdp_input::MouseButton::Left);
        up.click_count = Some(1);
        self.handle.execute(up, session).await?;
        Ok(())
    }

    /// Click at a specific point with custom button and click-count options.
    pub async fn click_with(
        &self,
        point: crate::layout::Point,
        options: &ClickOptions,
    ) -> crate::Result<()> {
        let session = Some(self.session_id.current());
        let count = options.click_count.max(1) as i64;
        let button = match &options.button {
            MouseButton::Right => cdp_input::MouseButton::Right,
            MouseButton::Middle => cdp_input::MouseButton::Middle,
            _ => cdp_input::MouseButton::Left,
        };
        let mut down = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MousePressed,
            point.x,
            point.y,
        );
        down.button = Some(button.clone());
        down.click_count = Some(count);
        self.handle.execute(down, session.clone()).await?;
        let mut up = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseReleased,
            point.x,
            point.y,
        );
        up.button = Some(button);
        up.click_count = Some(count);
        self.handle.execute(up, session).await?;
        Ok(())
    }

    /// Returns the stateful [`Mouse`] handle for this page.
    ///
    /// The returned handle tracks cursor position across calls. Clone it freely —
    /// all clones share the same position state.
    pub fn mouse(&self) -> Mouse {
        self.mouse.clone()
    }

    /// Returns the stateful [`Keyboard`] handle for this page.
    ///
    /// The returned handle tracks held modifier keys across calls. Clone it freely —
    /// all clones share the same modifier state.
    pub fn keyboard(&self) -> Keyboard {
        self.keyboard.clone()
    }

    /// High-level drag-and-drop between two CSS selectors.
    ///
    /// Finds each element, scrolls the source into view, then replays the
    /// mouse move → press → interpolated move → release sequence used by
    /// [`Element::drag_to`](crate::Element::drag_to).
    pub async fn drag_and_drop(
        &self,
        source: impl Into<String>,
        target: impl Into<String>,
    ) -> crate::Result<()> {
        let src_el = self.dom().find_element(&source.into()).await?;
        let dst_el = self.dom().find_element(&target.into()).await?;

        src_el.scroll_into_view().await.ok();
        dst_el.scroll_into_view().await.ok();

        let src = src_el.clickable_point().await?;
        let dst = dst_el.clickable_point().await?;
        let session = Some(self.session_id.current());

        let moved = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseMoved,
            src.x,
            src.y,
        );
        self.handle.execute(moved, session.clone()).await?;

        let mut pressed = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MousePressed,
            src.x,
            src.y,
        );
        pressed.button = Some(cdp_input::MouseButton::Left);
        pressed.click_count = Some(1);
        self.handle.execute(pressed, session.clone()).await?;

        let steps = 10i32;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let x = src.x + (dst.x - src.x) * t;
            let y = src.y + (dst.y - src.y) * t;
            let mut mv = cdp_input::DispatchMouseEventParams::new(
                cdp_input::DispatchMouseEventParamsType::MouseMoved,
                x,
                y,
            );
            mv.button = Some(cdp_input::MouseButton::Left);
            self.handle.execute(mv, session.clone()).await?;
        }

        let mut released = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseReleased,
            dst.x,
            dst.y,
        );
        released.button = Some(cdp_input::MouseButton::Left);
        released.click_count = Some(1);
        self.handle.execute(released, session).await?;
        Ok(())
    }
}
