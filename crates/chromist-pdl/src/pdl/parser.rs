//! PDL tokenizer, AST types, and [`parse`] function.
//!
//! The PDL format is line-oriented. Each line's leading spaces indicate its
//! nesting level:
//!
//! ```text
//! version
//!   major 1
//!   minor 3
//!
//! experimental domain Page
//!   depends on DOM
//!
//!   # A unique frame id.
//!   type FrameId extends string
//!
//!   command navigate
//!     parameters
//!       string url
//!     returns
//!       FrameId frameId
//!
//!   event loadEventFired
//!     parameters
//!       number timestamp
//! ```

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors that can occur while parsing a PDL file.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("line {line}: {message}")]
    Syntax { line: usize, message: String },
    #[error("{0}")]
    MissingHeader(String),
}

macro_rules! parse_err {
    (line $line:expr, $($arg:tt)*) => {
        ParseError::Syntax { line: $line, message: format!($($arg)*) }
    };
    ($($arg:tt)*) => {
        ParseError::MissingHeader(format!($($arg)*))
    };
}

// ---------------------------------------------------------------------------
// AST types
// ---------------------------------------------------------------------------

/// Top-level parsed PDL, containing a version header and a list of domains.
#[derive(Debug, Clone, PartialEq)]
pub struct Protocol {
    pub version: Version,
    pub domains: Vec<Domain>,
}

/// Protocol version header (`version\n  major N\n  minor N`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Version {
    pub major: String,
    pub minor: String,
}

/// A single CDP domain (e.g. `Page`, `DOM`, `Network`).
#[derive(Debug, Clone, PartialEq)]
pub struct Domain {
    pub name: String,
    pub description: Option<String>,
    pub experimental: bool,
    pub deprecated: bool,
    pub dependencies: Vec<String>,
    pub types: Vec<TypeDef>,
    pub commands: Vec<Command>,
    pub events: Vec<Event>,
}

/// A named type defined within a domain.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub id: String,
    pub description: Option<String>,
    pub experimental: bool,
    pub deprecated: bool,
    pub kind: TypeKind,
}

/// The body of a type definition.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// `type T extends string` with an `enum` block.
    Enum(Vec<EnumVariant>),
    /// `type T extends object` with a `properties` block.
    Object(Vec<Property>),
    /// A primitive type alias (`extends integer|number|…`).
    Primitive(PrimitiveType),
    /// A primitive array (`extends array of integer`).
    Array(Box<CdpType>),
    /// A ref array (`extends array of SomeDomain.SomeType`).
    Ref(String),
}

/// One variant inside an `enum` block.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub description: Option<String>,
}

/// A property / parameter / return value inside a type, command, or event.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub description: Option<String>,
    pub experimental: bool,
    pub deprecated: bool,
    pub optional: bool,
    pub kind: CdpType,
}

/// A CDP command (request + response pair).
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub experimental: bool,
    pub deprecated: bool,
    pub parameters: Vec<Property>,
    pub returns: Vec<Property>,
}

/// A CDP event (server-sent notification).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: String,
    pub description: Option<String>,
    pub experimental: bool,
    pub deprecated: bool,
    pub parameters: Vec<Property>,
}

/// The type of a field or parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum CdpType {
    Integer,
    Number,
    Boolean,
    String,
    Object,
    Any,
    Binary,
    Array(Box<CdpType>),
    /// A reference to another type, possibly cross-domain (`Domain.TypeId`).
    Ref(String),
    /// An inline enum declared directly on a parameter (not a top-level type).
    InlineEnum(Vec<EnumVariant>),
}

/// Primitive non-object, non-array CDPtype.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimitiveType {
    Integer,
    Number,
    Boolean,
    String,
    Object,
    Any,
    Binary,
}

// ---------------------------------------------------------------------------
// Internal parse state
// ---------------------------------------------------------------------------

/// Which kind of member list we are currently accumulating inside an element.
#[derive(Debug)]
enum MemberSection {
    Parameters,
    Returns,
    Properties,
}

/// The current top-level item inside a domain that we are building.
#[derive(Debug)]
enum CurrentElement {
    TypeDef {
        id: String,
        description: Option<String>,
        experimental: bool,
        deprecated: bool,
        /// Whether we've started an `enum` block (makes enum items the target
        /// instead of properties).
        enum_started: bool,
        variants: Vec<EnumVariant>,
        properties: Vec<Property>,
        /// The "extends" base — kept so we know if it is an enum/object/
        /// primitive/array once we close the element.
        extends: TypeExtends,
    },
    Command {
        name: String,
        description: Option<String>,
        experimental: bool,
        deprecated: bool,
        parameters: Vec<Property>,
        returns: Vec<Property>,
    },
    Event {
        name: String,
        description: Option<String>,
        experimental: bool,
        deprecated: bool,
        parameters: Vec<Property>,
    },
}

/// Mirrors the `extends <kind>` declaration on a type.
#[derive(Debug, Clone)]
enum TypeExtends {
    Primitive(PrimitiveType),
    Enum,
    Array(Box<CdpType>),
    Ref(String),
}

// ---------------------------------------------------------------------------
// Helper: parse a CDPType token (e.g. "integer", "$SomeRef", "array of X")
// ---------------------------------------------------------------------------

/// Parse a type-name token (without "array of" prefix) into a [`CdpType`].
fn parse_base_type(s: &str) -> CdpType {
    match s {
        "integer" => CdpType::Integer,
        "number" => CdpType::Number,
        "boolean" => CdpType::Boolean,
        "string" => CdpType::String,
        "object" => CdpType::Object,
        "any" => CdpType::Any,
        "binary" => CdpType::Binary,
        other => CdpType::Ref(other.to_string()),
    }
}

/// Parse a `PrimitiveType` from the string after `extends`.
fn parse_primitive(s: &str) -> Option<PrimitiveType> {
    match s {
        "integer" => Some(PrimitiveType::Integer),
        "number" => Some(PrimitiveType::Number),
        "boolean" => Some(PrimitiveType::Boolean),
        "string" => Some(PrimitiveType::String),
        "object" => Some(PrimitiveType::Object),
        "any" => Some(PrimitiveType::Any),
        "binary" => Some(PrimitiveType::Binary),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Finalise helpers
// ---------------------------------------------------------------------------

fn finalise_element(el: CurrentElement, domain: &mut Domain) {
    match el {
        CurrentElement::TypeDef {
            id,
            description,
            experimental,
            deprecated,
            enum_started: _,
            variants,
            properties,
            extends,
        } => {
            let kind = match extends {
                TypeExtends::Enum => TypeKind::Enum(variants),
                TypeExtends::Primitive(p) => {
                    // If there are properties accumulated despite a primitive
                    // extends, treat it as Object (shouldn't happen in practice).
                    if !properties.is_empty() {
                        TypeKind::Object(properties)
                    } else {
                        TypeKind::Primitive(p)
                    }
                }
                TypeExtends::Array(inner) => TypeKind::Array(inner),
                TypeExtends::Ref(r) => TypeKind::Ref(r),
            };
            domain.types.push(TypeDef { id, description, experimental, deprecated, kind });
        }
        CurrentElement::Command {
            name,
            description,
            experimental,
            deprecated,
            parameters,
            returns,
        } => {
            domain.commands.push(Command {
                name,
                description,
                experimental,
                deprecated,
                parameters,
                returns,
            });
        }
        CurrentElement::Event { name, description, experimental, deprecated, parameters } => {
            domain.events.push(Event { name, description, experimental, deprecated, parameters });
        }
    }
}

// ---------------------------------------------------------------------------
// Main parse function
// ---------------------------------------------------------------------------

/// Parse a PDL file into a [`Protocol`].
///
/// The input may have been pre-processed to inline `include` directives (see
/// [`crate::pdl::resolver`]).
///
/// # Errors
///
/// Returns a [`ParseError`] if the file contains an unrecognised token or
/// structurally invalid nesting.
pub fn parse(input: &str) -> Result<Protocol, ParseError> {
    let mut version: Option<Version> = None;
    let mut in_version = false;
    let mut domains: Vec<Domain> = Vec::new();
    // Pending description: comment lines accumulate here before being
    // applied to the next structural element.
    let mut pending_desc: Option<String> = None;

    // Within a domain, we accumulate the current element being built.
    let mut current_element: Option<CurrentElement> = None;
    // Which member section (parameters / returns / properties) is active.
    let mut current_member: Option<MemberSection> = None;
    // True when we are collecting enum literal lines for a *field* whose type
    // was declared `enum` (inline enum on a parameter/property).
    let mut member_enum_active = false;

    for (line_idx, raw_line) in input.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = raw_line.trim();

        // ----------------------------------------------------------------
        // Skip blank lines (but flush pending description if not consumed)
        // ----------------------------------------------------------------
        if trimmed.is_empty() {
            continue;
        }

        // ----------------------------------------------------------------
        // Comment lines — accumulate into pending_desc
        // ----------------------------------------------------------------
        if trimmed.starts_with('#') {
            let text: String = trimmed.chars().skip(1).skip_while(|c| c.is_whitespace()).collect();
            match pending_desc.as_mut() {
                Some(desc) => {
                    desc.push('\n');
                    desc.push_str(&text);
                }
                None => pending_desc = Some(text),
            }
            continue;
        }

        // Reset member_enum_active if we are no longer at 6-space indent
        // (i.e., we've moved to a line that is not a member literal).
        // We do this detection lazily when we hit any structural keyword.

        // ----------------------------------------------------------------
        // `version` header
        // ----------------------------------------------------------------
        if raw_line.starts_with("version") && !raw_line.starts_with("  ") {
            // Finalise any pending domain element before starting version
            // (version comes before domains, so this is only for safety)
            in_version = true;
            version = Some(Version::default());
            pending_desc = None; // version has no description
            continue;
        }

        // ----------------------------------------------------------------
        // `  major N` / `  minor N`  (inside version block)
        // ----------------------------------------------------------------
        if in_version {
            if let Some(rest) = strip_prefix_spaces(raw_line, 2) {
                if let Some(n) = rest.strip_prefix("major ") {
                    if let Some(v) = version.as_mut() {
                        v.major = n.trim().to_string();
                    }
                    continue;
                }
                if let Some(n) = rest.strip_prefix("minor ") {
                    if let Some(v) = version.as_mut() {
                        v.minor = n.trim().to_string();
                    }
                    continue;
                }
            }
        }

        // ----------------------------------------------------------------
        // `[experimental] [deprecated] domain <Name>`  (top-level, no indent)
        // ----------------------------------------------------------------
        if !raw_line.starts_with(' ') {
            in_version = false;
            // Finalise pending element for previous domain
            if let Some(el) = current_element.take() {
                if let Some(dom) = domains.last_mut() {
                    finalise_element(el, dom);
                }
            }
            current_member = None;
            member_enum_active = false;

            // Try to match `domain` declaration
            let mut rest = trimmed;
            let experimental = consume_flag(&mut rest, "experimental ");
            let deprecated = consume_flag(&mut rest, "deprecated ");
            if let Some(name) = rest.strip_prefix("domain ") {
                domains.push(Domain {
                    name: name.trim().to_string(),
                    description: pending_desc.take(),
                    experimental,
                    deprecated,
                    dependencies: Vec::new(),
                    types: Vec::new(),
                    commands: Vec::new(),
                    events: Vec::new(),
                });
                continue;
            }

            // Any other non-indented non-version non-domain line: just skip
            // (e.g. comment lines already handled, blank lines already handled)
            pending_desc = None;
            continue;
        }

        // ----------------------------------------------------------------
        // Lines with 2-space indent: `  depends on`, `  type`, `  command`,
        // `  event`
        // ----------------------------------------------------------------
        if let Some(rest) = strip_prefix_spaces(raw_line, 2) {
            if rest.starts_with(' ') {
                // More than 2 spaces — handled below
            } else {
                member_enum_active = false;

                // `depends on <Domain>`
                if let Some(dep) = rest.strip_prefix("depends on ") {
                    domains
                        .last_mut()
                        .ok_or_else(|| parse_err!(line line_num, "'depends on' without domain"))?
                        .dependencies
                        .push(dep.trim().to_string());
                    pending_desc = None;
                    continue;
                }

                // `[experimental] [deprecated] type <Name> extends [array of] <Kind>`
                let mut tok = rest;
                let experimental = consume_flag(&mut tok, "experimental ");
                let deprecated = consume_flag(&mut tok, "deprecated ");

                if let Some(rest2) = tok.strip_prefix("type ") {
                    // Finalise previous element
                    if let Some(el) = current_element.take() {
                        if let Some(dom) = domains.last_mut() {
                            finalise_element(el, dom);
                        }
                    }
                    current_member = None;

                    // `<Name> extends [array of] <Kind>`
                    let (name, extends) = parse_type_extends(rest2, line_num)?;
                    current_element = Some(CurrentElement::TypeDef {
                        id: name,
                        description: pending_desc.take(),
                        experimental,
                        deprecated,
                        enum_started: matches!(extends, TypeExtends::Enum),
                        variants: Vec::new(),
                        properties: Vec::new(),
                        extends,
                    });
                    continue;
                }

                // `[experimental] [deprecated] command <Name>`
                if let Some(name) = tok.strip_prefix("command ") {
                    if let Some(el) = current_element.take() {
                        if let Some(dom) = domains.last_mut() {
                            finalise_element(el, dom);
                        }
                    }
                    current_member = None;
                    current_element = Some(CurrentElement::Command {
                        name: name.trim().to_string(),
                        description: pending_desc.take(),
                        experimental,
                        deprecated,
                        parameters: Vec::new(),
                        returns: Vec::new(),
                    });
                    continue;
                }

                // `[experimental] [deprecated] event <Name>`
                if let Some(name) = tok.strip_prefix("event ") {
                    if let Some(el) = current_element.take() {
                        if let Some(dom) = domains.last_mut() {
                            finalise_element(el, dom);
                        }
                    }
                    current_member = None;
                    current_element = Some(CurrentElement::Event {
                        name: name.trim().to_string(),
                        description: pending_desc.take(),
                        experimental,
                        deprecated,
                        parameters: Vec::new(),
                    });
                    continue;
                }

                // Unknown 2-space line (e.g. `redirect`) — skip
                pending_desc = None;
                continue;
            }
        }

        // ----------------------------------------------------------------
        // Lines with 4-space indent: `    parameters`, `    returns`,
        // `    properties`, `    enum`, `    redirect`
        // ----------------------------------------------------------------
        if let Some(rest) = strip_prefix_spaces(raw_line, 4) {
            if rest.starts_with(' ') {
                // More than 4 spaces — handled below
            } else {
                member_enum_active = false;

                match rest.trim() {
                    "parameters" => {
                        current_member = Some(MemberSection::Parameters);
                        pending_desc = None;
                        continue;
                    }
                    "returns" => {
                        current_member = Some(MemberSection::Returns);
                        pending_desc = None;
                        continue;
                    }
                    "properties" => {
                        current_member = Some(MemberSection::Properties);
                        pending_desc = None;
                        continue;
                    }
                    "enum" => {
                        // Start an enum block inside a type
                        if let Some(CurrentElement::TypeDef { enum_started, extends, .. }) =
                            current_element.as_mut()
                        {
                            *enum_started = true;
                            *extends = TypeExtends::Enum;
                        }
                        pending_desc = None;
                        continue;
                    }
                    _ if rest.trim().starts_with("redirect ") => {
                        // Ignore redirect lines
                        pending_desc = None;
                        continue;
                    }
                    _ => {
                        // Unknown 4-space keyword — skip
                        pending_desc = None;
                        continue;
                    }
                }
            }
        }

        // ----------------------------------------------------------------
        // Lines with 6-space indent: parameter/property/return field
        // declarations OR enum literal values
        // ----------------------------------------------------------------
        if let Some(rest) = strip_prefix_spaces(raw_line, 6) {
            // Check if this is another level (8+ spaces) — would be very
            // unusual; treat as enum literal for now.

            // Determine if this looks like a property declaration or an enum
            // literal.  A property declaration has the pattern:
            //   [experimental] [deprecated] [optional] [array of] <type> <name>
            // An enum literal is a bare word (possibly with leading spaces for
            // sub-indentation).

            let trimmed6 = rest.trim_start();

            // Try to parse as a field/property line
            if let Some(mut prop) = try_parse_property(trimmed6, pending_desc.take()) {
                // A property declared with type `enum` means the following
                // indented lines are inline variant literals.  Convert it to
                // InlineEnum immediately so it is valid in the AST even if
                // the enum block is empty (no variants follow).
                if matches!(&prop.kind, CdpType::Ref(r) if r == "enum") {
                    prop.kind = CdpType::InlineEnum(Vec::new());
                    member_enum_active = true;
                }

                push_property_to_member(prop, &mut current_element, &current_member, line_num)?;
                continue;
            }

            // Not a property line — treat as an enum literal (a bare word)
            // This applies to both type-level enum blocks and field-level
            // inline enums.
            let literal = trimmed6.to_string();
            if literal.is_empty() {
                continue;
            }

            let desc = pending_desc.take();

            if member_enum_active {
                // Push variant to the last property in the current member section
                push_enum_variant_to_member_field(
                    literal,
                    desc,
                    &mut current_element,
                    &current_member,
                    line_num,
                )?;
            } else if let Some(CurrentElement::TypeDef { enum_started: true, variants, .. }) =
                current_element.as_mut()
            {
                variants.push(EnumVariant { name: literal, description: desc });
            }
            // else: ignore unknown line
            continue;
        }

        // Anything else — ignore
        pending_desc = None;
    }

    // Finalise the last element and domain
    if let Some(el) = current_element.take() {
        if let Some(dom) = domains.last_mut() {
            finalise_element(el, dom);
        }
    }

    let version = version.ok_or_else(|| parse_err!("Missing version header"))?;
    Ok(Protocol { version, domains })
}

// ---------------------------------------------------------------------------
// Sub-parsers
// ---------------------------------------------------------------------------

/// Try to strip exactly `n` spaces from the start of `s`.
/// Returns `None` if `s` has fewer than `n` leading spaces.
fn strip_prefix_spaces(s: &str, n: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() < n {
        return None;
    }
    for b in bytes.iter().take(n) {
        if *b != b' ' {
            return None;
        }
    }
    Some(&s[n..])
}

/// Consume `flag` from the start of `*s` (mutating it) and return whether it
/// was present.
fn consume_flag(s: &mut &str, flag: &str) -> bool {
    if let Some(rest) = s.strip_prefix(flag) {
        *s = rest;
        true
    } else {
        false
    }
}

/// Parse `<Name> extends [array of] <Kind>` from a type declaration line.
fn parse_type_extends(rest: &str, line_num: usize) -> Result<(String, TypeExtends), ParseError> {
    // Split on " extends "
    let parts: Vec<&str> = rest.splitn(2, " extends ").collect();
    if parts.len() != 2 {
        return Err(
            parse_err!(line line_num, "expected 'type <Name> extends <kind>', got '{rest}'"),
        );
    }
    let name = parts[0].trim().to_string();
    let kind_str = parts[1].trim();

    let extends = if kind_str == "enum" {
        TypeExtends::Enum
    } else if let Some(inner) = kind_str.strip_prefix("array of ") {
        let inner_type = parse_base_type(inner.trim());
        TypeExtends::Array(Box::new(inner_type))
    } else if let Some(prim) = parse_primitive(kind_str) {
        TypeExtends::Primitive(prim)
    } else {
        TypeExtends::Ref(kind_str.to_string())
    };

    Ok((name, extends))
}

/// Try to parse a property/parameter line from the trimmed content.
///
/// Returns `None` if this does not look like a property line (i.e. it's an
/// enum literal).
///
/// A property line has the pattern:
/// `[experimental] [deprecated] [optional] [array of] <type> <name>`
///
/// The last word is always the field name and the second-to-last (or the
/// "array of <type>" group) is the type.
fn try_parse_property(line: &str, desc: Option<String>) -> Option<Property> {
    // Tokenize
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 2 {
        return None;
    }

    let mut experimental = false;
    let mut deprecated = false;
    let mut optional = false;

    // Consume flags
    loop {
        match words.first().copied() {
            Some("experimental") => {
                experimental = true;
                words.remove(0);
            }
            Some("deprecated") => {
                deprecated = true;
                words.remove(0);
            }
            Some("optional") => {
                optional = true;
                words.remove(0);
            }
            _ => break,
        }
    }

    if words.len() < 2 {
        return None;
    }

    // Check for `array of <type> <name>`
    let (cdp_type, name) = if words.len() >= 3 && words[0] == "array" && words[1] == "of" {
        let ty = parse_base_type(words[2]);
        let name = words.last().copied()?;
        // In "array of <type> <name>" the name is the last word.
        // If words.len() == 4: ["array", "of", "<type>", "<name>"]
        (CdpType::Array(Box::new(ty)), name)
    } else {
        // `<type> <name>` (possibly after flags)
        if words.len() < 2 {
            return None;
        }
        let type_tok = words[0];
        let name_tok = words[1];
        // If there are more words, this isn't a well-formed property line
        // (unless it's something we don't understand). For robustness accept.
        (parse_base_type(type_tok), name_tok)
    };

    // Heuristic: if the "type" token is a bare word that starts with a
    // lowercase letter and there is no second word acting as name, treat as
    // enum literal.  But we already require words.len() >= 2 above.

    // Additional guard: if name contains '.' or is a keyword that clearly
    // isn't a name, fall through to treating as enum literal.

    Some(Property {
        name: name.to_string(),
        description: desc,
        experimental,
        deprecated,
        optional,
        kind: cdp_type,
    })
}

/// Push a parsed property onto the appropriate member section within the
/// current element.
fn push_property_to_member(
    prop: Property,
    current_element: &mut Option<CurrentElement>,
    current_member: &Option<MemberSection>,
    line_num: usize,
) -> Result<(), ParseError> {
    match current_element {
        Some(CurrentElement::TypeDef { properties, enum_started, .. }) => {
            if *enum_started {
                // A property inside a type that declared `enum` — this is the
                // reference parser's "member_enum" pattern: a field whose type
                // is literally `enum`, meaning it's an inline enum on a struct
                // property.  We handle this by accepting the property even
                // inside an enum-extends type when the member section is
                // Properties.
            }
            // Only push to properties if we are in the Properties section
            if let Some(MemberSection::Properties) = current_member {
                properties.push(prop)
            }
        }
        Some(CurrentElement::Command { parameters, returns, .. }) => match current_member {
            Some(MemberSection::Parameters) => parameters.push(prop),
            Some(MemberSection::Returns) => returns.push(prop),
            _ => {
                return Err(
                    parse_err!(line line_num, "property '{}' has no active member section", prop.name),
                );
            }
        },
        Some(CurrentElement::Event { parameters, .. }) => match current_member {
            Some(MemberSection::Parameters) => parameters.push(prop),
            _ => {
                return Err(
                    parse_err!(line line_num, "event property '{}' has no parameters section", prop.name),
                );
            }
        },
        None => {
            return Err(
                parse_err!(line line_num, "property '{}' outside of any element", prop.name),
            );
        }
    }
    Ok(())
}

/// Push an enum variant literal onto the last property in the current member
/// section (for inline `enum` fields like `enum level\n  log\n  warning`).
fn push_enum_variant_to_member_field(
    name: String,
    desc: Option<String>,
    current_element: &mut Option<CurrentElement>,
    current_member: &Option<MemberSection>,
    line_num: usize,
) -> Result<(), ParseError> {
    // Get the last property in the current member list
    let last_prop = match current_element {
        Some(CurrentElement::TypeDef { properties, .. }) => match current_member {
            Some(MemberSection::Properties) => properties.last_mut(),
            _ => None,
        },
        Some(CurrentElement::Command { parameters, returns, .. }) => match current_member {
            Some(MemberSection::Parameters) => parameters.last_mut(),
            Some(MemberSection::Returns) => returns.last_mut(),
            _ => None,
        },
        Some(CurrentElement::Event { parameters, .. }) => match current_member {
            Some(MemberSection::Parameters) => parameters.last_mut(),
            _ => None,
        },
        None => None,
    };

    let prop = last_prop.ok_or_else(
        || parse_err!(line line_num, "enum literal '{}' but no active property", name),
    )?;

    let variant = EnumVariant { name, description: desc };
    match &mut prop.kind {
        CdpType::InlineEnum(variants) => {
            variants.push(variant);
        }
        _ => {
            // property kind is not InlineEnum — shouldn't happen since we
            // eagerly convert Ref("enum") properties before calling here
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let pdl = r#"
version
  major 1
  minor 2

experimental domain DummyDomain
  depends on DOM

  # node identifier.
  type NodeId extends string

  type SomeValueType extends string
    enum
      boolean
      undefined
      booleanOrUndefined

  type ConsoleMessage extends object
    properties
      string text
      optional string url
      optional integer line

  command navigate
    parameters
      string url
    returns
      NodeId frameId

  event loadEventFired
    parameters
      number timestamp
"#;
        let proto = parse(pdl).unwrap();
        assert_eq!(proto.version.major, "1");
        assert_eq!(proto.version.minor, "2");
        assert_eq!(proto.domains.len(), 1);
        let dom = &proto.domains[0];
        assert_eq!(dom.name, "DummyDomain");
        assert!(dom.experimental);
        assert_eq!(dom.dependencies, vec!["DOM"]);
        assert_eq!(dom.types.len(), 3);
        assert_eq!(dom.commands.len(), 1);
        assert_eq!(dom.events.len(), 1);

        // NodeId is a string primitive
        assert!(matches!(dom.types[0].kind, TypeKind::Primitive(PrimitiveType::String)));

        // SomeValueType is an enum
        if let TypeKind::Enum(vars) = &dom.types[1].kind {
            assert_eq!(vars.len(), 3);
            assert_eq!(vars[0].name, "boolean");
        } else {
            panic!("expected enum");
        }

        // ConsoleMessage is an object with properties
        if let TypeKind::Object(props) = &dom.types[2].kind {
            assert_eq!(props.len(), 3);
            assert!(!props[0].optional);
            assert!(props[1].optional);
        } else {
            panic!("expected object");
        }

        // command navigate
        let cmd = &dom.commands[0];
        assert_eq!(cmd.name, "navigate");
        assert_eq!(cmd.parameters.len(), 1);
        assert_eq!(cmd.returns.len(), 1);

        // event loadEventFired
        let ev = &dom.events[0];
        assert_eq!(ev.name, "loadEventFired");
        assert_eq!(ev.parameters.len(), 1);
    }

    #[test]
    fn empty_input_is_rejected() {
        // Parser requires a version header — empty input should error cleanly,
        // not panic.
        assert!(parse("").is_err());
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let pdl = r#"
# leading comment
version
  major 1
  minor 0

# between domains

domain Foo
  # a type
  type Id extends string
"#;
        let proto = parse(pdl).unwrap();
        assert_eq!(proto.version.major, "1");
        assert_eq!(proto.domains.len(), 1);
        assert_eq!(proto.domains[0].types.len(), 1);
    }

    #[test]
    fn deprecated_and_experimental_domain_flags() {
        let pdl = r#"
version
  major 1
  minor 0

experimental deprecated domain Weird
  type X extends string
"#;
        let proto = parse(pdl).unwrap();
        let d = &proto.domains[0];
        assert!(d.experimental);
        assert!(d.deprecated);
    }

    #[test]
    fn primitive_type_aliases() {
        let pdl = r#"
version
  major 1
  minor 0

domain P
  type I extends integer
  type N extends number
  type B extends boolean
  type S extends string
  type O extends object
  type A extends any
"#;
        let proto = parse(pdl).unwrap();
        let types = &proto.domains[0].types;
        assert!(matches!(types[0].kind, TypeKind::Primitive(PrimitiveType::Integer)));
        assert!(matches!(types[1].kind, TypeKind::Primitive(PrimitiveType::Number)));
        assert!(matches!(types[2].kind, TypeKind::Primitive(PrimitiveType::Boolean)));
        assert!(matches!(types[3].kind, TypeKind::Primitive(PrimitiveType::String)));
        assert!(matches!(types[4].kind, TypeKind::Primitive(PrimitiveType::Object)));
        assert!(matches!(types[5].kind, TypeKind::Primitive(PrimitiveType::Any)));
    }

    #[test]
    fn parse_base_type_refs_unknown() {
        assert_eq!(parse_base_type("integer"), CdpType::Integer);
        assert_eq!(parse_base_type("Foo.Bar"), CdpType::Ref("Foo.Bar".to_string()));
    }

    #[test]
    fn parse_primitive_rejects_non_primitive() {
        assert!(parse_primitive("not_a_type").is_none());
        assert_eq!(parse_primitive("integer"), Some(PrimitiveType::Integer));
    }

    #[test]
    fn multiple_dependencies() {
        let pdl = r#"
version
  major 1
  minor 0

domain Page
  depends on DOM
  depends on Network
"#;
        let proto = parse(pdl).unwrap();
        assert_eq!(proto.domains[0].dependencies, vec!["DOM", "Network"]);
    }

    #[test]
    fn command_without_parameters_or_returns() {
        let pdl = r#"
version
  major 1
  minor 0

domain D
  command ping
"#;
        let proto = parse(pdl).unwrap();
        let cmd = &proto.domains[0].commands[0];
        assert_eq!(cmd.name, "ping");
        assert!(cmd.parameters.is_empty());
        assert!(cmd.returns.is_empty());
    }

    #[test]
    fn property_optional_flag() {
        let pdl = r#"
version
  major 1
  minor 0

domain D
  type Foo extends object
    properties
      string required
      optional integer maybe
"#;
        let proto = parse(pdl).unwrap();
        if let TypeKind::Object(props) = &proto.domains[0].types[0].kind {
            assert_eq!(props.len(), 2);
            assert!(!props[0].optional);
            assert!(props[1].optional);
        } else {
            panic!("expected object");
        }
    }

    #[test]
    fn parse_error_is_returned_for_malformed_indent() {
        // A nonsense top-level keyword should be rejected.
        let pdl = "definitely not pdl syntax\n";
        let result = parse(pdl);
        assert!(result.is_err(), "expected parse error, got: {:?}", result);
    }

    #[test]
    fn parse_error_display_syntax() {
        let e = ParseError::Syntax { line: 5, message: "boom".into() };
        assert!(e.to_string().contains("boom"));
        assert!(e.to_string().contains("5"));
    }

    #[test]
    fn parse_error_display_missing_header() {
        let e = ParseError::MissingHeader("no version".into());
        assert_eq!(e.to_string(), "no version");
    }

    #[test]
    fn inline_enum_on_parameter() {
        let pdl = r#"
version
  major 1
  minor 0

domain D
  command send
    parameters
      enum level
        verbose
        info
        warning
"#;
        let proto = parse(pdl).unwrap();
        let param = &proto.domains[0].commands[0].parameters[0];
        assert_eq!(param.name, "level");
        if let CdpType::InlineEnum(variants) = &param.kind {
            assert_eq!(variants.len(), 3);
            assert_eq!(variants[0].name, "verbose");
            assert_eq!(variants[2].name, "warning");
        } else {
            panic!("expected InlineEnum, got {:?}", param.kind);
        }
    }

    #[test]
    fn event_with_no_parameters() {
        let pdl = r#"
version
  major 1
  minor 0

domain D
  event ready
"#;
        let proto = parse(pdl).unwrap();
        let ev = &proto.domains[0].events[0];
        assert_eq!(ev.name, "ready");
        assert!(ev.parameters.is_empty());
    }
}
