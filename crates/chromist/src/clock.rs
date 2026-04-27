//! Browser clock control via an injected JavaScript fake-timers shim.
//!
//! Install the shim on a page with [`Page::install_clock`], then obtain a
//! [`Clock`] handle from [`Page::clock`] to freeze, tick, or restore time.

use crate::page::Page;

/// JavaScript source for the minimal fake-timers shim.
///
/// Replaces `Date`, `Date.now`, `performance.now`, `setTimeout`,
/// `setInterval`, `clearTimeout`, and `clearInterval` with controllable
/// implementations stored under `window.__chromistClock`.
const CLOCK_SHIM: &str = r#"(function() {
  if (window.__chromistClock) return;
  var _origDate = Date;
  var _origSetTimeout = setTimeout;
  var _origSetInterval = setInterval;
  var _origClearTimeout = clearTimeout;
  var _origClearInterval = clearInterval;
  var _origPerfNow = performance.now.bind(performance);
  var _now = _origDate.now();
  var _pageOrigin = _origPerfNow();
  var _timers = [];
  var _nextId = 1;
  function _install(fn, ms, repeat) {
    var id = _nextId++;
    var deadline = _now + (ms || 0);
    _timers.push({id: id, fn: fn, deadline: deadline, ms: ms || 0, repeat: repeat});
    _timers.sort(function(a, b) { return a.deadline - b.deadline; });
    return id;
  }
  function FakeDate() {
    if (!(this instanceof FakeDate)) { return new FakeDate().toString(); }
    var d = arguments.length === 0 ? new _origDate(_now) : new (Function.prototype.bind.apply(_origDate, [null].concat(Array.prototype.slice.call(arguments))))();
    return d;
  }
  FakeDate.prototype = _origDate.prototype;
  FakeDate.now = function() { return _now; };
  FakeDate.parse = _origDate.parse.bind(_origDate);
  FakeDate.UTC = _origDate.UTC.bind(_origDate);
  window.Date = FakeDate;
  window.setTimeout = function(fn, ms) { return _install(fn, ms, false); };
  window.setInterval = function(fn, ms) { return _install(fn, ms, true); };
  window.clearTimeout = window.clearInterval = function(id) {
    _timers = _timers.filter(function(t) { return t.id !== id; });
  };
  Object.defineProperty(performance, 'now', {
    configurable: true,
    value: function() { return _now - _pageOrigin; }
  });
  window.__chromistClock = {
    setTime: function(ms) { _now = ms; },
    tick: function(ms) {
      var target = _now + ms;
      while (_timers.length && _timers[0].deadline <= target) {
        var t = _timers.shift();
        _now = t.deadline;
        try { t.fn(); } catch(e) {}
        if (t.repeat) {
          t.deadline = _now + t.ms;
          _timers.push(t);
          _timers.sort(function(a, b) { return a.deadline - b.deadline; });
        }
      }
      _now = target;
    },
    restore: function() {
      window.Date = _origDate;
      window.setTimeout = _origSetTimeout;
      window.setInterval = _origSetInterval;
      window.clearTimeout = _origClearTimeout;
      window.clearInterval = _origClearInterval;
      Object.defineProperty(performance, 'now', { configurable: true, value: _origPerfNow });
      delete window.__chromistClock;
    }
  };
})()"#;

/// Handle for controlling a page's fake clock.
///
/// Obtain one by calling [`Page::clock`] after installing the shim with
/// [`Page::install_clock`].
#[derive(Debug, Clone)]
pub struct Clock {
    page: Page,
}

impl Clock {
    pub(crate) fn new(page: Page) -> Self {
        Self { page }
    }

    /// Set the clock to an absolute epoch millisecond value without advancing
    /// pending timers.
    pub async fn set_time(&self, millis_since_epoch: u64) -> crate::Result<()> {
        let js = format!(
            "window.__chromistClock && window.__chromistClock.setTime({})",
            millis_since_epoch
        );
        self.page.evaluate(&js).await?;
        Ok(())
    }

    /// Freeze time at the given epoch millisecond value.
    ///
    /// Equivalent to `set_time` — the clock won't advance on its own until
    /// `tick` or `restore` is called.
    pub async fn freeze(&self, millis_since_epoch: u64) -> crate::Result<()> {
        self.set_time(millis_since_epoch).await
    }

    /// Advance the clock by `ms` milliseconds, firing any pending timers that
    /// fall within the interval.
    pub async fn tick(&self, ms: u64) -> crate::Result<()> {
        let js = format!("window.__chromistClock && window.__chromistClock.tick({})", ms);
        self.page.evaluate(&js).await?;
        Ok(())
    }

    /// Restore the original `Date`, `setTimeout`, and `setInterval` globals.
    pub async fn restore(&self) -> crate::Result<()> {
        self.page.evaluate("window.__chromistClock && window.__chromistClock.restore()").await?;
        Ok(())
    }

    /// Advance the clock by `ms` milliseconds, firing pending timers.
    ///
    /// Alias for [`tick`](Clock::tick).
    pub async fn fast_forward(&self, ms: u64) -> crate::Result<()> {
        self.tick(ms).await
    }

    /// Set the clock to an absolute epoch millisecond value.
    ///
    /// Alias for [`set_time`](Clock::set_time).
    pub async fn set_system_time(&self, millis_since_epoch: u64) -> crate::Result<()> {
        self.set_time(millis_since_epoch).await
    }
}

impl Page {
    /// Inject the fake-timers shim into the page and return a [`Clock`] handle.
    ///
    /// This must be called *after* the page has navigated (or use
    /// [`add_init_script`] to inject it before navigation).  Calling this
    /// method a second time on the same page is a no-op (the shim guards
    /// against double-installation).
    ///
    /// [`add_init_script`]: Page::add_init_script
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let clock = page.install_clock().await?;
    /// clock.freeze(1_700_000_000_000).await?;
    /// // … page now sees Date.now() == 1_700_000_000_000
    /// clock.tick(5_000).await?;
    /// clock.restore().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn install_clock(&self) -> crate::Result<Clock> {
        self.evaluate(CLOCK_SHIM).await?;
        Ok(Clock::new(self.clone()))
    }

    /// Return a [`Clock`] handle without re-injecting the shim.
    ///
    /// Use this after [`install_clock`] was already called (e.g., in tests
    /// that do a one-time setup).
    ///
    /// [`install_clock`]: Page::install_clock
    pub fn clock(&self) -> Clock {
        Clock::new(self.clone())
    }
}
