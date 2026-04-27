//! PDL AST, parser, resolver, and dependency ordering.

pub mod dep;
pub mod parser;
pub mod resolver;

pub use parser::{
    CdpType, Command, Domain, EnumVariant, Event, ParseError, PrimitiveType, Property, Protocol,
    TypeDef, TypeKind, Version,
};

/// Parse a PDL string (with `include` directives already resolved) into a
/// [`Protocol`].
///
/// This is the primary public entry point for the parser.
///
/// # Errors
///
/// Returns a [`ParseError`] describing the first unrecognised or malformed
/// token encountered.
///
/// # Example
///
/// ```rust,no_run
/// let content = std::fs::read_to_string("browser_protocol.pdl").unwrap();
/// let protocol = chromist_pdl::pdl::parse(&content).unwrap();
/// println!("{} domains", protocol.domains.len());
/// ```
pub fn parse(input: &str) -> Result<Protocol, ParseError> {
    parser::parse(input)
}
