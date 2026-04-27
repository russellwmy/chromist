//! Property-based round-trip tests for the PDL parser.
//!
//! For each generated [`Protocol`], we render it back to canonical PDL source
//! and then re-parse it. The property under test is:
//!
//! ```text
//! parse(render(p)) == p
//! ```
//!
//! Coverage is intentionally limited to AST shapes the parser canonically
//! supports (no inline-enum properties, no descriptions — the latter are
//! lossy through the comment-trim pipeline). Within that frame the strategy
//! exercises every kind of `TypeDef` (primitive, enum, object, array, ref),
//! every primitive `CdpType`, optional / experimental / deprecated flags on
//! types, properties, commands, and events, and dependency lists.
//!
//! A failing case shrinks down to the minimal AST shape that breaks the
//! round-trip — far more useful for diagnosing parser regressions than a
//! fixture-only suite.

use chromist_pdl::pdl::parser::{
    CdpType, Command, Domain, Event, Property, Protocol, TypeDef, TypeKind, Version,
};
use chromist_pdl::pdl::{parse, EnumVariant, PrimitiveType};
use proptest::collection::vec;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

fn render(p: &Protocol) -> String {
    let mut s = String::new();
    s.push_str("version\n");
    s.push_str(&format!("  major {}\n", p.version.major));
    s.push_str(&format!("  minor {}\n", p.version.minor));
    for d in &p.domains {
        s.push('\n');
        render_domain(&mut s, d);
    }
    s
}

fn render_flags(s: &mut String, experimental: bool, deprecated: bool) {
    if experimental {
        s.push_str("experimental ");
    }
    if deprecated {
        s.push_str("deprecated ");
    }
}

fn render_domain(s: &mut String, d: &Domain) {
    render_flags(s, d.experimental, d.deprecated);
    s.push_str(&format!("domain {}\n", d.name));
    for dep in &d.dependencies {
        s.push_str(&format!("  depends on {dep}\n"));
    }
    for t in &d.types {
        render_type(s, t);
    }
    for c in &d.commands {
        render_command(s, c);
    }
    for e in &d.events {
        render_event(s, e);
    }
}

fn cdp_type_token(t: &CdpType) -> String {
    match t {
        CdpType::Integer => "integer".into(),
        CdpType::Number => "number".into(),
        CdpType::Boolean => "boolean".into(),
        CdpType::String => "string".into(),
        CdpType::Object => "object".into(),
        CdpType::Any => "any".into(),
        CdpType::Binary => "binary".into(),
        CdpType::Ref(r) => r.clone(),
        // Arrays / inline enums are rendered specially by callers.
        CdpType::Array(_) | CdpType::InlineEnum(_) => unreachable!("rendered by caller"),
    }
}

fn primitive_token(p: &PrimitiveType) -> &'static str {
    match p {
        PrimitiveType::Integer => "integer",
        PrimitiveType::Number => "number",
        PrimitiveType::Boolean => "boolean",
        PrimitiveType::String => "string",
        PrimitiveType::Object => "object",
        PrimitiveType::Any => "any",
        PrimitiveType::Binary => "binary",
    }
}

fn render_type(s: &mut String, t: &TypeDef) {
    s.push_str("  ");
    render_flags(s, t.experimental, t.deprecated);
    match &t.kind {
        TypeKind::Primitive(p) => {
            s.push_str(&format!("type {} extends {}\n", t.id, primitive_token(p)));
        }
        TypeKind::Enum(variants) => {
            // `extends string` + indented `enum` block — most permissive form
            // that round-trips cleanly through the parser's enum-block path.
            s.push_str(&format!("type {} extends string\n", t.id));
            s.push_str("    enum\n");
            for v in variants {
                s.push_str(&format!("      {}\n", v.name));
            }
        }
        TypeKind::Object(props) => {
            s.push_str(&format!("type {} extends object\n", t.id));
            if !props.is_empty() {
                s.push_str("    properties\n");
                for p in props {
                    render_property(s, p);
                }
            }
        }
        TypeKind::Array(inner) => {
            s.push_str(&format!("type {} extends array of {}\n", t.id, cdp_type_token(inner)));
        }
        TypeKind::Ref(r) => {
            s.push_str(&format!("type {} extends {r}\n", t.id));
        }
    }
}

fn render_property(s: &mut String, p: &Property) {
    s.push_str("      ");
    render_flags(s, p.experimental, p.deprecated);
    if p.optional {
        s.push_str("optional ");
    }
    match &p.kind {
        CdpType::Array(inner) => {
            s.push_str(&format!("array of {} {}\n", cdp_type_token(inner), p.name));
        }
        other => {
            s.push_str(&format!("{} {}\n", cdp_type_token(other), p.name));
        }
    }
}

fn render_command(s: &mut String, c: &Command) {
    s.push_str("  ");
    render_flags(s, c.experimental, c.deprecated);
    s.push_str(&format!("command {}\n", c.name));
    if !c.parameters.is_empty() {
        s.push_str("    parameters\n");
        for p in &c.parameters {
            render_property(s, p);
        }
    }
    if !c.returns.is_empty() {
        s.push_str("    returns\n");
        for p in &c.returns {
            render_property(s, p);
        }
    }
}

fn render_event(s: &mut String, e: &Event) {
    s.push_str("  ");
    render_flags(s, e.experimental, e.deprecated);
    s.push_str(&format!("event {}\n", e.name));
    if !e.parameters.is_empty() {
        s.push_str("    parameters\n");
        for p in &e.parameters {
            render_property(s, p);
        }
    }
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

// Identifiers limited to ASCII alpha[num] starting with a letter. Reserved
// keywords are filtered to avoid producing PDL that the parser would
// disambiguate as a different token (`enum`, `object`, primitives, …).
const RESERVED: &[&str] = &[
    "version",
    "domain",
    "depends",
    "type",
    "command",
    "event",
    "parameters",
    "returns",
    "properties",
    "enum",
    "extends",
    "experimental",
    "deprecated",
    "optional",
    "array",
    "of",
    "on",
    "redirect",
    "integer",
    "number",
    "boolean",
    "string",
    "object",
    "any",
    "binary",
    "major",
    "minor",
];

fn ident_strategy(prefix: &'static str) -> impl Strategy<Value = String> {
    // 1–6 trailing alphanumerics on top of a leading letter; reject reserved
    // identifiers to keep PDL output unambiguous.
    "[a-zA-Z][a-zA-Z0-9]{0,5}"
        .prop_filter("reserved", |s: &String| !RESERVED.contains(&s.as_str()))
        .prop_map(move |s| format!("{prefix}{s}"))
}

fn type_name() -> impl Strategy<Value = String> {
    // PascalCase-ish via prefix, plus filter against reserved.
    ident_strategy("T")
}

fn field_name() -> impl Strategy<Value = String> {
    ident_strategy("f")
}

fn command_name() -> impl Strategy<Value = String> {
    ident_strategy("c")
}

fn event_name() -> impl Strategy<Value = String> {
    ident_strategy("e")
}

fn domain_name() -> impl Strategy<Value = String> {
    ident_strategy("D")
}

fn version_part() -> impl Strategy<Value = String> {
    "[0-9]{1,3}".prop_map(|s: String| s)
}

fn primitive_strategy() -> impl Strategy<Value = PrimitiveType> {
    prop_oneof![
        Just(PrimitiveType::Integer),
        Just(PrimitiveType::Number),
        Just(PrimitiveType::Boolean),
        Just(PrimitiveType::String),
        Just(PrimitiveType::Object),
        Just(PrimitiveType::Any),
        Just(PrimitiveType::Binary),
    ]
}

// `CdpType` excluding arrays and inline enums — used for property leaves and
// for the inner type of arrays.
fn leaf_cdp_type() -> impl Strategy<Value = CdpType> {
    prop_oneof![
        Just(CdpType::Integer),
        Just(CdpType::Number),
        Just(CdpType::Boolean),
        Just(CdpType::String),
        Just(CdpType::Object),
        Just(CdpType::Any),
        Just(CdpType::Binary),
        type_name().prop_map(CdpType::Ref),
    ]
}

fn property_cdp_type() -> impl Strategy<Value = CdpType> {
    prop_oneof![leaf_cdp_type(), leaf_cdp_type().prop_map(|t| CdpType::Array(Box::new(t))),]
}

prop_compose! {
    fn property_strategy()(
        name in field_name(),
        experimental in any::<bool>(),
        deprecated in any::<bool>(),
        optional in any::<bool>(),
        kind in property_cdp_type(),
    ) -> Property {
        Property {
            name,
            description: None,
            experimental,
            deprecated,
            optional,
            kind,
        }
    }
}

prop_compose! {
    fn enum_variant_strategy()(name in ident_strategy("V")) -> EnumVariant {
        EnumVariant { name, description: None }
    }
}

fn type_kind_strategy() -> impl Strategy<Value = TypeKind> {
    prop_oneof![
        primitive_strategy().prop_map(TypeKind::Primitive),
        vec(enum_variant_strategy(), 1..4).prop_map(TypeKind::Enum),
        // `Object` must carry ≥1 property: the parser cannot distinguish
        // `extends object` with zero properties from `Primitive(Object)`,
        // so an empty object is not round-trippable (parser collapses to
        // primitive). The case is covered by `Primitive(Object)` instead.
        vec(property_strategy(), 1..4).prop_map(TypeKind::Object),
        leaf_cdp_type().prop_map(|t| TypeKind::Array(Box::new(t))),
        type_name().prop_map(TypeKind::Ref),
    ]
}

prop_compose! {
    fn type_def_strategy()(
        id in type_name(),
        experimental in any::<bool>(),
        deprecated in any::<bool>(),
        kind in type_kind_strategy(),
    ) -> TypeDef {
        TypeDef { id, description: None, experimental, deprecated, kind }
    }
}

prop_compose! {
    fn command_strategy()(
        name in command_name(),
        experimental in any::<bool>(),
        deprecated in any::<bool>(),
        parameters in vec(property_strategy(), 0..4),
        returns in vec(property_strategy(), 0..4),
    ) -> Command {
        Command { name, description: None, experimental, deprecated, parameters, returns }
    }
}

prop_compose! {
    fn event_strategy()(
        name in event_name(),
        experimental in any::<bool>(),
        deprecated in any::<bool>(),
        parameters in vec(property_strategy(), 0..4),
    ) -> Event {
        Event { name, description: None, experimental, deprecated, parameters }
    }
}

prop_compose! {
    fn domain_strategy()(
        name in domain_name(),
        experimental in any::<bool>(),
        deprecated in any::<bool>(),
        dependencies in vec(domain_name(), 0..3),
        types in vec(type_def_strategy(), 0..3),
        commands in vec(command_strategy(), 0..3),
        events in vec(event_strategy(), 0..3),
    ) -> Domain {
        Domain {
            name,
            description: None,
            experimental,
            deprecated,
            dependencies,
            types,
            commands,
            events,
        }
    }
}

prop_compose! {
    fn protocol_strategy()(
        major in version_part(),
        minor in version_part(),
        domains in vec(domain_strategy(), 0..4),
    ) -> Protocol {
        Protocol { version: Version { major, minor }, domains }
    }
}

// ---------------------------------------------------------------------------
// The round-trip property
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        // Larger generated trees take noticeably longer; keep the failure
        // cap modest so a failure shrinks quickly.
        max_shrink_iters: 1024,
        ..ProptestConfig::default()
    })]

    #[test]
    fn parse_render_roundtrip(p in protocol_strategy()) {
        let src = render(&p);
        match parse(&src) {
            Ok(parsed) => {
                prop_assert_eq!(
                    &parsed, &p,
                    "round-trip mismatch.\n--- rendered PDL ---\n{}\n--- expected AST ---\n{:#?}\n--- parsed AST ---\n{:#?}",
                    src, p, parsed
                );
            }
            Err(e) => {
                prop_assert!(
                    false,
                    "parser rejected rendered output: {e}\n--- rendered PDL ---\n{src}"
                );
            }
        }
    }

    /// A weaker corollary: even when comparison fails, parsing must succeed
    /// — a successful render should never produce syntactically invalid PDL.
    #[test]
    fn rendered_output_always_parses(p in protocol_strategy()) {
        let src = render(&p);
        prop_assert!(
            parse(&src).is_ok(),
            "parse failed on rendered output:\n{src}"
        );
    }
}
