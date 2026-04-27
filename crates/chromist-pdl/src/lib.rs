//! PDL parser and Rust code generator for the Chrome DevTools Protocol.
//!
//! # Overview
//!
//! This crate provides two main capabilities:
//!
//! 1. **Parsing** — [`pdl::parse`] reads a `.pdl` file and produces an owned
//!    AST ([`pdl::Protocol`]) describing all domains, types, commands, and
//!    events.
//!
//! 2. **Code generation** — [`build::Generator`] accepts one or more `.pdl`
//!    files and returns a `String` of valid Rust source that can be written to
//!    `cdp.rs` and included via `include!`.

pub mod build;
pub mod pdl;
