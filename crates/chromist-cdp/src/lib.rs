//! Generated CDP bindings for the Chrome DevTools Protocol.
//!
//! The main entry point is the [`cdp`] module which contains all generated
//! domain modules and the global [`cdp::events::CdpEvent`] enum.
//!
//! # Revision
//!
//! These bindings were generated from the PDL files pinned at Chromium commit
//! position [`PROTOCOL_REVISION`].  The PDL files track the `devtools-protocol`
//! repository at <https://github.com/ChromeDevTools/devtools-protocol>.
//!
//! # Feature Flags
//!
//! | Flag | Default | Description |
//! |------|---------|-------------|
//! | `tracing` | no | Enables a `tracing::warn!` log whenever `CdpEventMessage` encounters an unrecognised method name during deserialization. Useful for detecting protocol-version skew between the browser and the vendored PDL files at runtime. |
//!
//! Enable it in `Cargo.toml`:
//!
//! ```toml
//! chromist-cdp = { version = "0.1", features = ["tracing"] }
//! ```
//!
//! With the feature enabled, unknown events emit:
//!
//! ```text
//! WARN chromist_cdp: unrecognized CDP event; consider upgrading chromist-cdp method="Future.newDomain"
//! ```

// Scoped allows for the generated file. Each lint below fires somewhere in the
// output of `chromist-pdl`. Blanket `#[allow(clippy::all)]` is avoided so that
// future lints the generator does *not* trip still produce warnings.
#[allow(clippy::derive_partial_eq_without_eq)]
#[allow(clippy::large_enum_variant)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::enum_variant_names)]
#[allow(clippy::module_name_repetitions)]
#[allow(clippy::doc_markdown)]
#[allow(clippy::empty_docs)]
#[allow(clippy::wrong_self_convention)]
#[allow(unreachable_patterns)]
#[allow(deprecated)]
#[rustfmt::skip]
pub mod cdp;

/// The Chromium commit position (monotonic) at which the vendored PDL files
/// were pinned.
///
/// See <https://chromiumdash.appspot.com/commits> to map it to a browser version.
pub const PROTOCOL_REVISION: &str = "1619965";
