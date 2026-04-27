//! Code generator for Chrome DevTools Protocol PDL files.

mod builder;
mod event;
mod generator;
mod types;

pub use generator::{Generator, GeneratorError};
