pub(crate) const ELEMENT_INNER_TEXT: &str = r#"
function() { return this.innerText; }
"#;

pub(crate) const ELEMENT_INNER_HTML: &str = "function() { return this.innerHTML; }";
pub(crate) const ELEMENT_OUTER_HTML: &str = "function() { return this.outerHTML; }";

pub(crate) const ELEMENT_TEXT_CONTENT: &str = r#"
function() { return this.textContent; }
"#;

pub(crate) const CLICKABLE_POINT: &str = r#"
function() {
    this.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
    const rect = this.getBoundingClientRect();
    return { x: rect.left + rect.width/2, y: rect.top + rect.height/2 };
}
"#;
pub(crate) const GET_DOCUMENT_CONTENT: &str = r#"
(() => {
    let s = '';
    if (document.doctype) {
        const d = document.doctype;
        s = '<!DOCTYPE ' + d.name;
        if (d.publicId) s += ' PUBLIC "' + d.publicId + '"';
        if (d.systemId) s += ' "' + d.systemId + '"';
        s += '>';
    }
    s += document.documentElement.outerHTML;
    return s;
})()
"#;

pub(crate) const STEALTH_HIDE_WEBDRIVER: &str = r#"
Object.defineProperty(Object.getPrototypeOf(navigator), 'webdriver', { get: () => false });
"#;

pub(crate) const STEALTH_CHROME_RUNTIME: &str = r#"
window.chrome = {
    app: { isInstalled: false, InstallState: { DISABLED:'disabled',INSTALLED:'installed',NOT_INSTALLED:'not_installed' }, RunningState: { CANNOT_RUN:'cannot_run',READY_TO_RUN:'ready_to_run',RUNNING:'running' } },
    csi: function(){},
    loadTimes: function(){},
    runtime: {}
};
"#;

pub(crate) const STEALTH_PERMISSIONS: &str = r#"
const originalQuery = window.navigator.permissions.query;
window.navigator.permissions.query = (parameters) =>
    parameters.name === 'notifications'
        ? Promise.resolve({ state: Notification.permission })
        : originalQuery(parameters);
"#;

pub(crate) const STEALTH_WEBGL_VENDOR: &str = r#"
const getParameter = WebGLRenderingContext.getParameter.bind(WebGLRenderingContext.prototype);
WebGLRenderingContext.prototype.getParameter = function(parameter) {
    if (parameter === 37445) return 'Intel Inc.';
    if (parameter === 37446) return 'Intel Iris OpenGL Engine';
    return getParameter(parameter);
};
"#;

pub(crate) const STEALTH_PLUGINS: &str = r#"
Object.defineProperty(navigator, 'plugins', { get: () => [1,2,3,4,5] });
Object.defineProperty(navigator, 'languages', { get: () => ['en-US','en'] });
"#;

// ── Actionability helpers ──────────────────────────────────────────────────

/// Returns `true` if the element has a non-zero bounding box, is not
/// `visibility:hidden`, `display:none`, or `opacity:0`, and is connected.
pub(crate) const IS_VISIBLE: &str = r#"
function() {
    if (!this.isConnected) return false;
    const r = this.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return false;
    const s = window.getComputedStyle(this);
    if (s.visibility === 'hidden' || s.display === 'none') return false;
    if (parseFloat(s.opacity) === 0) return false;
    return true;
}
"#;

/// Samples `getBoundingClientRect` twice, 50 ms apart.  Returns `true` when
/// the rect is identical — meaning the element has stopped moving.
pub(crate) const IS_STABLE: &str = r#"
async function() {
    const r1 = this.getBoundingClientRect();
    await new Promise(r => setTimeout(r, 50));
    const r2 = this.getBoundingClientRect();
    return r1.top === r2.top && r1.left === r2.left &&
           r1.width === r2.width && r1.height === r2.height;
}
"#;

/// Returns `true` if the element at the center of this element's bounding
/// box is this element or one of its descendants (i.e. nothing is obscuring it).
pub(crate) const HIT_TEST: &str = r#"
function() {
    const r = this.getBoundingClientRect();
    const el = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!el && (this === el || this.contains(el));
}
"#;

/// Returns `true` when the element is not `disabled` and has no
/// `aria-disabled="true"` attribute.
pub(crate) const IS_ENABLED: &str = r#"
function() {
    if (this.disabled) return false;
    if (this.getAttribute('aria-disabled') === 'true') return false;
    return true;
}
"#;

/// Logical inverse of [`IS_VISIBLE`] — returns `true` when the element is
/// hidden, detached, zero-size, or has `visibility:hidden`/`display:none`/`opacity:0`.
pub(crate) const IS_HIDDEN: &str = r#"
function() {
    if (!this.isConnected) return true;
    const r = this.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return true;
    const s = window.getComputedStyle(this);
    if (s.visibility === 'hidden' || s.display === 'none') return true;
    if (parseFloat(s.opacity) === 0) return true;
    return false;
}
"#;

/// Returns `true` when the element's `checked` property is truthy.
pub(crate) const IS_CHECKED: &str = r#"
function() { return !!this.checked; }
"#;

/// Returns `true` when the element is disabled (via the `disabled` property
/// or `aria-disabled="true"`).
pub(crate) const IS_DISABLED: &str = r#"
function() {
    return !!this.disabled || this.getAttribute('aria-disabled') === 'true';
}
"#;

/// Returns `true` when the element can accept text input — not disabled, not
/// read-only, and is an `<input>`, `<textarea>`, or `contenteditable` element.
pub(crate) const IS_EDITABLE: &str = r#"
function() {
    if (this.disabled) return false;
    if (this.readOnly) return false;
    const tag = this.tagName.toLowerCase();
    return tag === 'input' || tag === 'textarea' || this.contentEditable === 'true';
}
"#;
