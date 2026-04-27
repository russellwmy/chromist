//! The main code generator.
//!
//! [`Generator::compile_pdls`] reads one or more `.pdl` files, parses them,
//! and emits a single `String` of valid Rust source.

use std::collections::HashSet;
use std::path::PathBuf;

use heck::{ToSnakeCase, ToUpperCamelCase};
use indexmap::IndexMap;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::build::builder::{field_ident, FieldSpec, StructBuilder};
use crate::build::event::{generate_domain_event_enum, generate_global_event_enum};
use crate::build::types::{map_cdp_type, resolve_ref_type, subenum_name};
use crate::pdl::parser::{
    CdpType, Command, Domain, Event, PrimitiveType, Property, Protocol, TypeDef, TypeKind,
};
use crate::pdl::resolver::read_pdl;

/// Errors produced by the code generator.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: crate::pdl::parser::ParseError,
    },
    #[error("{0}")]
    Other(String),
}

impl From<String> for GeneratorError {
    fn from(s: String) -> Self {
        GeneratorError::Other(s)
    }
}

/// Rust code generator for Chrome DevTools Protocol PDL files.
///
/// # Example
///
/// ```no_run
/// use chromist_pdl::build::Generator;
/// use std::path::PathBuf;
///
/// let code = Generator::default()
///     .compile_pdls(&[PathBuf::from("browser_protocol.pdl")])
///     .expect("code generation failed");
/// std::fs::write("cdp.rs", code).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct Generator {
    out_dir: Option<PathBuf>,
}

impl Generator {
    /// Create a generator that writes output to `path`.
    pub fn out_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.out_dir = Some(path.into());
        self
    }

    /// Parse the given `.pdl` files and return the generated Rust as a
    /// `String`.
    ///
    /// The returned string contains a single top-level source file with one
    /// `pub mod <domain_snake>` per CDP domain, plus an `events` module.
    ///
    /// # Errors
    ///
    /// Returns [`GeneratorError::Io`] if a file cannot be read, or
    /// [`GeneratorError::Parse`] if a file contains invalid PDL syntax.
    pub fn compile_pdls(&self, pdl_files: &[PathBuf]) -> Result<String, GeneratorError> {
        if pdl_files.is_empty() {
            return Err(GeneratorError::Other("no PDL files provided".into()));
        }

        // Step 1: parse all PDL files
        let mut protocols: Vec<Protocol> = Vec::new();
        let mut protocol_mods: Vec<String> = Vec::new();
        // Map from domain name → index of the protocol (PDL file) it lives in
        let mut domains: IndexMap<String, usize> = IndexMap::new();
        // Set of fully-qualified enum type names ("DomainName.TypeId")
        let mut enum_types: HashSet<String> = HashSet::new();

        for path in pdl_files {
            let stem = path
                .file_stem()
                .ok_or_else(|| {
                    GeneratorError::Other(format!("no file stem for {}", path.display()))
                })?
                .to_string_lossy()
                .into_owned();

            let content = read_pdl(path).map_err(|source| GeneratorError::Parse {
                path: path.display().to_string(),
                source,
            })?;
            let protocol = crate::pdl::parser::parse(&content).map_err(|source| {
                GeneratorError::Parse { path: path.display().to_string(), source }
            })?;

            let pdl_idx = protocols.len();

            for domain in &protocol.domains {
                domains.insert(domain.name.clone(), pdl_idx);

                // Collect enum types
                for ty in &domain.types {
                    if matches!(ty.kind, TypeKind::Enum(_)) {
                        enum_types.insert(format!("{}.{}", domain.name, ty.id));
                    }
                }
            }

            protocol_mods.push(stem);
            protocols.push(protocol);
        }

        let ctx = GenContext {
            domains: &domains,
            protocol_mods: &protocol_mods,
            enum_types: &enum_types,
        };

        // Step 2: generate one sub-module per PDL file
        let mut pdl_modules = TokenStream::default();

        for (pdl_idx, protocol) in protocols.iter().enumerate() {
            let mod_name = format_ident!("{}", &protocol_mods[pdl_idx]);
            let version_str = format!("{}.{}", protocol.version.major, protocol.version.minor);

            let domain_modules = generate_domains(&protocol.domains, pdl_idx, &ctx);

            pdl_modules.extend(quote! {
                pub mod #mod_name {
                    /// Version of this protocol definition.
                    pub const VERSION: &str = #version_str;
                    #domain_modules
                }
            });
        }

        // Step 3: collect all events for the global CdpEvent enum.
        // Pre-compute snake_case domain names into owned Strings so we can
        // borrow them below without leaking memory on each compile_pdls call.
        let domain_snakes: Vec<Vec<String>> = protocols
            .iter()
            .map(|p| p.domains.iter().map(|d| d.name.to_snake_case()).collect())
            .collect();
        let all_events: Vec<(&str, &str, &Event)> = protocols
            .iter()
            .zip(domain_snakes.iter())
            .flat_map(|(p, snakes)| {
                p.domains.iter().zip(snakes.iter()).flat_map(|(d, snake)| {
                    d.events.iter().map(move |ev| (d.name.as_str(), snake.as_str(), ev))
                })
            })
            .collect();

        let events_module_content =
            generate_global_event_enum(&all_events, &protocol_mods, &domains);

        // Step 4: assemble the final output
        let stream = quote! {
            /// Serde helper functions used by generated enum fields.
            pub mod de {
                use std::str::FromStr;
                use serde::Deserialize;

                /// Deserialize a string via `FromStr` (non-optional field).
                pub fn deserialize_from_str<'de, T, D>(deserializer: D) -> Result<T, D::Error>
                where
                    T: FromStr,
                    T::Err: std::fmt::Display,
                    D: serde::Deserializer<'de>,
                {
                    let s = String::deserialize(deserializer)?;
                    T::from_str(&s).map_err(serde::de::Error::custom)
                }

                /// Deserialize an optional string via `FromStr`.
                pub fn deserialize_from_str_optional<'de, T, D>(
                    deserializer: D,
                ) -> Result<Option<T>, D::Error>
                where
                    T: FromStr,
                    T::Err: std::fmt::Display,
                    D: serde::Deserializer<'de>,
                {
                    let opt = Option::<String>::deserialize(deserializer)?;
                    match opt {
                        None => Ok(None),
                        Some(s) if s.is_empty() => Ok(None),
                        Some(s) => T::from_str(&s).map(Some).map_err(serde::de::Error::custom),
                    }
                }
            }

            #[allow(unused_imports)]
            pub mod events {
                use serde::Deserialize as _;
                #events_module_content
            }

            #pdl_modules
        };

        Ok(prettyprint(stream))
    }
}

// ---------------------------------------------------------------------------
// Generation context — shared across all domain/type generators
// ---------------------------------------------------------------------------

struct GenContext<'a> {
    /// domain name → PDL file index
    domains: &'a IndexMap<String, usize>,
    /// ordered list of PDL module names
    protocol_mods: &'a [String],
    /// fully-qualified enum type names ("DomainName.TypeId")
    enum_types: &'a HashSet<String>,
}

// ---------------------------------------------------------------------------
// Domain-level generation
// ---------------------------------------------------------------------------

fn generate_domains(domains: &[Domain], pdl_idx: usize, ctx: &GenContext) -> TokenStream {
    let mut stream = TokenStream::default();

    for domain in domains {
        let doc = doc_attr(domain.description.as_deref());
        let deprecated = if domain.deprecated {
            quote! { #[deprecated] }
        } else {
            quote! {}
        };
        let mod_name = format_ident!("{}", domain.name.to_snake_case());
        let domain_content = generate_domain(domain, pdl_idx, ctx);

        stream.extend(quote! {
            #doc
            #deprecated
            pub mod #mod_name {
                #domain_content
            }
        });
    }

    stream
}

fn generate_domain(domain: &Domain, pdl_idx: usize, ctx: &GenContext) -> TokenStream {
    let mut stream = quote! {
        use serde::{Serialize, Deserialize};
    };

    // Types
    for ty in &domain.types {
        stream.extend(generate_typedef(ty, &domain.name, pdl_idx, ctx));
    }

    // Commands
    for cmd in &domain.commands {
        stream.extend(generate_command(cmd, &domain.name, pdl_idx, ctx));
    }

    // Events
    for ev in &domain.events {
        stream.extend(generate_event(ev, &domain.name, pdl_idx, ctx));
    }

    // Domain-level Event enum
    stream.extend(generate_domain_event_enum(&domain.name, &domain.events));

    stream
}

// ---------------------------------------------------------------------------
// TypeDef generation
// ---------------------------------------------------------------------------

fn generate_typedef(ty: &TypeDef, domain: &str, pdl_idx: usize, ctx: &GenContext) -> TokenStream {
    let doc = doc_attr(ty.description.as_deref());
    let deprecated_attr = if ty.deprecated {
        quote! { #[deprecated] }
    } else {
        quote! {}
    };

    match &ty.kind {
        TypeKind::Enum(variants) => {
            let enum_ts = generate_enum_type(&ty.id, ty.description.as_deref(), variants);
            quote! {
                #deprecated_attr
                #enum_ts
            }
        }
        TypeKind::Object(props) => {
            let struct_ts = generate_object_type(
                &ty.id,
                ty.description.as_deref(),
                props,
                domain,
                pdl_idx,
                ctx,
            );
            quote! {
                #deprecated_attr
                #struct_ts
            }
        }
        TypeKind::Primitive(prim) => {
            let rust_ty = primitive_to_tokens(prim);
            let name = format_ident!("{}", ty.id.to_upper_camel_case());
            let mut stream = quote! {
                #doc
                #deprecated_attr
                #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
                pub struct #name(pub #rust_ty);

                impl #name {
                    pub fn new(val: impl Into<#rust_ty>) -> Self {
                        Self(val.into())
                    }
                    pub fn inner(&self) -> &#rust_ty {
                        &self.0
                    }
                }
            };

            // Add Eq + Hash for integer and string primitives
            match prim {
                PrimitiveType::Integer => {
                    stream.extend(quote! {
                        impl Eq for #name {}
                        impl std::hash::Hash for #name {
                            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                                self.0.hash(state);
                            }
                        }
                        impl Copy for #name {}
                    });
                }
                PrimitiveType::String => {
                    stream.extend(quote! {
                        impl Eq for #name {}
                        impl std::hash::Hash for #name {
                            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                                self.0.hash(state);
                            }
                        }
                        impl AsRef<str> for #name {
                            fn as_ref(&self) -> &str { self.0.as_str() }
                        }
                        impl From<String> for #name {
                            fn from(s: String) -> Self { #name(s) }
                        }
                        impl From<#name> for String {
                            fn from(v: #name) -> String { v.0 }
                        }
                    });
                    if ty.id.ends_with("Id") {
                        stream.extend(quote! {
                            impl std::borrow::Borrow<str> for #name {
                                fn borrow(&self) -> &str { &self.0 }
                            }
                        });
                    }
                }
                _ => {}
            }

            stream
        }
        TypeKind::Array(inner) => {
            let name = format_ident!("{}", ty.id.to_upper_camel_case());
            let inner_ts =
                map_cdp_type(inner, domain, &ty.id, pdl_idx, ctx.domains, ctx.protocol_mods)
                    .to_token_stream();
            quote! {
                #doc
                #deprecated_attr
                #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
                pub struct #name(pub Vec<#inner_ts>);
            }
        }
        TypeKind::Ref(name_ref) => {
            let name = format_ident!("{}", ty.id.to_upper_camel_case());
            let target =
                resolve_ref_type(name_ref, domain, pdl_idx, ctx.domains, ctx.protocol_mods);
            quote! {
                #doc
                #deprecated_attr
                pub type #name = #target;
            }
        }
    }
}

fn generate_enum_type(
    id: &str,
    desc: Option<&str>,
    variants: &[crate::pdl::parser::EnumVariant],
) -> TokenStream {
    let name = format_ident!("{}", id.to_upper_camel_case());
    let doc = doc_attr(desc);

    // Collect variant tokens
    let vars: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let var_name = format_ident!("{}", enum_variant_name(&v.name));
            let rename = &v.name;
            let vdoc = doc_attr(v.description.as_deref());
            quote! {
                #vdoc
                #[serde(rename = #rename)]
                #var_name
            }
        })
        .collect();

    let var_idents: Vec<_> =
        variants.iter().map(|v| format_ident!("{}", enum_variant_name(&v.name))).collect();
    let str_values: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();

    // as_ref → str
    let as_ref_arms: Vec<TokenStream> =
        var_idents.iter().zip(str_values.iter()).map(|(v, s)| quote! { #name::#v => #s }).collect();

    // from_str
    let from_str_arms: Vec<TokenStream> = var_idents
        .iter()
        .zip(str_values.iter())
        .flat_map(|(v, s)| {
            let lower = s.to_lowercase();
            let camel = enum_variant_name(s);
            let mut patterns = vec![s.to_string()];
            if camel != *s && !patterns.contains(&camel) {
                patterns.push(camel);
            }
            if !patterns.contains(&lower) {
                patterns.push(lower);
            }
            let v = v.clone();
            let name = name.clone();
            let ts: TokenStream = quote! {
                #(#patterns)|* => Ok(#name::#v),
            };
            std::iter::once(ts)
        })
        .collect();

    quote! {
        #doc
        #[non_exhaustive]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum #name {
            #(#vars,)*
        }

        impl AsRef<str> for #name {
            fn as_ref(&self) -> &str {
                match self {
                    #(#as_ref_arms,)*
                }
            }
        }

        impl ::std::str::FromStr for #name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    #(#from_str_arms)*
                    _ => Err(s.to_string()),
                }
            }
        }
    }
}

fn generate_object_type(
    id: &str,
    desc: Option<&str>,
    props: &[Property],
    domain: &str,
    pdl_idx: usize,
    ctx: &GenContext,
) -> TokenStream {
    let name_str = id.to_upper_camel_case();
    let name = format_ident!("{}", name_str);
    let doc = doc_attr(desc);

    let (fields, inline_enums) = build_fields(props, &name_str, domain, pdl_idx, ctx);

    let has_mandatory = fields.iter().any(|f| !f.optional);
    let derives = if has_mandatory {
        quote! { #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] }
    } else {
        quote! { #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)] }
    };

    let mut sb = StructBuilder::new(name);
    sb.fields = fields;

    let struct_def = sb.generate_struct_def();
    let impl_def = sb.generate_impl();

    quote! {
        #inline_enums
        #doc
        #derives
        #struct_def
        #impl_def
    }
}

// ---------------------------------------------------------------------------
// Command generation
// ---------------------------------------------------------------------------

fn generate_command(cmd: &Command, domain: &str, pdl_idx: usize, ctx: &GenContext) -> TokenStream {
    let camel = cmd.name.to_upper_camel_case();
    let params_name = format_ident!("{}Params", camel);
    let response_name = format_ident!("{}Response", camel);
    let identifier = format!("{}.{}", domain, cmd.name);
    let doc = doc_attr(cmd.description.as_deref());
    let deprecated_attr = if cmd.deprecated {
        quote! { #[deprecated] }
    } else {
        quote! {}
    };

    // Params struct
    let (param_fields, inline_enums) =
        build_fields(&cmd.parameters, &format!("{}Params", camel), domain, pdl_idx, ctx);
    let has_mandatory_params = param_fields.iter().any(|f| !f.optional);
    let param_derives = if has_mandatory_params {
        quote! { #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] }
    } else {
        quote! { #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)] }
    };

    let mut params_sb = StructBuilder::new(params_name.clone());
    params_sb.fields = param_fields;
    let params_struct = params_sb.generate_struct_def();
    let params_impl = params_sb.generate_impl();

    // Response struct
    let (resp_fields, resp_inline_enums) =
        build_fields(&cmd.returns, &format!("{}Response", camel), domain, pdl_idx, ctx);
    let has_mandatory_resp = resp_fields.iter().any(|f| !f.optional);
    let resp_derives = if has_mandatory_resp {
        quote! { #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] }
    } else {
        quote! { #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)] }
    };

    let mut resp_sb = StructBuilder::new(response_name.clone());
    resp_sb.fields = resp_fields;
    let resp_struct = resp_sb.generate_struct_def();

    let stream = quote! {
        #inline_enums
        #resp_inline_enums
        #doc
        #deprecated_attr
        #param_derives
        #params_struct
        #params_impl

        impl #params_name {
            pub const IDENTIFIER: &'static str = #identifier;
        }

        impl chromist_types::Method for #params_name {
            fn identifier(&self) -> chromist_types::MethodId {
                std::borrow::Cow::Borrowed(Self::IDENTIFIER)
            }
        }

        impl chromist_types::MethodType for #params_name {
            fn method_id() -> chromist_types::MethodId {
                std::borrow::Cow::Borrowed(Self::IDENTIFIER)
            }
        }

        impl chromist_types::Command for #params_name {
            type Response = #response_name;
        }

        #resp_derives
        #resp_struct
    };

    stream
}

// ---------------------------------------------------------------------------
// Event generation
// ---------------------------------------------------------------------------

fn generate_event(ev: &Event, domain: &str, pdl_idx: usize, ctx: &GenContext) -> TokenStream {
    let camel = ev.name.to_upper_camel_case();
    let struct_name = format_ident!("{}Event", camel);
    let identifier = format!("{}.{}", domain, ev.name);
    let doc = doc_attr(ev.description.as_deref());
    let deprecated_attr = if ev.deprecated {
        quote! { #[deprecated] }
    } else {
        quote! {}
    };

    let (fields, inline_enums) =
        build_fields(&ev.parameters, &format!("{}Event", camel), domain, pdl_idx, ctx);
    let has_mandatory = fields.iter().any(|f| !f.optional);
    let derives = if has_mandatory {
        quote! { #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] }
    } else {
        quote! { #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)] }
    };

    let mut sb = StructBuilder::new(struct_name.clone());
    sb.fields = fields;
    let struct_def = sb.generate_struct_def();

    quote! {
        #inline_enums
        #doc
        #deprecated_attr
        #derives
        #struct_def

        impl #struct_name {
            pub const IDENTIFIER: &'static str = #identifier;
        }

        impl chromist_types::Method for #struct_name {
            fn identifier(&self) -> chromist_types::MethodId {
                std::borrow::Cow::Borrowed(Self::IDENTIFIER)
            }
        }

        impl chromist_types::MethodType for #struct_name {
            fn method_id() -> chromist_types::MethodId {
                std::borrow::Cow::Borrowed(Self::IDENTIFIER)
            }
        }

        impl chromist_types::EventMessage for #struct_name {}
    }
}

// ---------------------------------------------------------------------------
// Field building helpers
// ---------------------------------------------------------------------------

/// Build `FieldSpec` list from a `&[Property]`, also returning any inline
/// enum definitions needed.
fn build_fields(
    props: &[Property],
    parent_camel: &str,
    domain: &str,
    pdl_idx: usize,
    ctx: &GenContext,
) -> (Vec<FieldSpec>, TokenStream) {
    let mut fields = Vec::new();
    let mut inline_enums = TokenStream::default();

    for prop in props {
        // Detect inline enums: CdpType::Ref("__enum__:…") sentinel
        let (actual_kind, maybe_inline_enum) =
            resolve_inline_enum(&prop.kind, parent_camel, &prop.name);

        if let Some(enum_ts) = maybe_inline_enum {
            inline_enums.extend(enum_ts);
        }

        let is_enum = is_enum_type(&actual_kind, domain, ctx);

        let rt = map_cdp_type(
            &actual_kind,
            domain,
            parent_camel,
            pdl_idx,
            ctx.domains,
            ctx.protocol_mods,
        );

        // Extra serde attributes for enum types
        let extra_serde = if is_enum {
            if prop.optional {
                quote! {
                    #[serde(default)]
                    #[serde(deserialize_with = "super::super::de::deserialize_from_str_optional")]
                }
            } else {
                quote! {
                    #[serde(deserialize_with = "super::super::de::deserialize_from_str")]
                }
            }
        } else {
            TokenStream::default()
        };

        fields.push(FieldSpec {
            cdp_name: prop.name.clone(),
            rust_name: field_ident(&prop.name),
            base_ty: rt.inner,
            is_vec: rt.is_vec,
            needs_box: rt.needs_box,
            optional: prop.optional,
            doc: prop.description.clone(),
            serde_skip: false,
            extra_serde,
        });
    }

    (fields, inline_enums)
}

/// Check if a `CdpType` ultimately refers to an enum type.
fn is_enum_type(ty: &CdpType, domain: &str, ctx: &GenContext) -> bool {
    match ty {
        CdpType::InlineEnum(_) => true,
        CdpType::Ref(name) => {
            // Qualified: "Domain.TypeId"
            if name.contains('.') {
                ctx.enum_types.contains(name)
            } else {
                ctx.enum_types.contains(&format!("{domain}.{name}"))
            }
        }
        _ => false,
    }
}

/// If `kind` is `CdpType::InlineEnum`, generate the enum definition and return
/// a `CdpType::Ref` to the generated type name.
fn resolve_inline_enum(
    kind: &CdpType,
    parent_camel: &str,
    field_name: &str,
) -> (CdpType, Option<TokenStream>) {
    if let CdpType::InlineEnum(variants) = kind {
        let enum_name = subenum_name(parent_camel, field_name);
        let enum_ts = generate_enum_type(&enum_name, None, variants);
        return (CdpType::Ref(enum_name), Some(enum_ts));
    }
    (kind.clone(), None)
}

// ---------------------------------------------------------------------------
// Enum variant name helper
// ---------------------------------------------------------------------------

fn enum_variant_name(name: &str) -> String {
    match name {
        "Self" | "self" => "KSelf".to_string(),
        _ => name.to_upper_camel_case(),
    }
}

// ---------------------------------------------------------------------------
// Primitive type → TokenStream
// ---------------------------------------------------------------------------

fn primitive_to_tokens(p: &PrimitiveType) -> TokenStream {
    match p {
        PrimitiveType::Integer => quote! { i64 },
        PrimitiveType::Number => quote! { f64 },
        PrimitiveType::Boolean => quote! { bool },
        PrimitiveType::String => quote! { String },
        PrimitiveType::Object | PrimitiveType::Any => quote! { serde_json::Value },
        PrimitiveType::Binary => quote! { chromist_types::Binary },
    }
}

// ---------------------------------------------------------------------------
// Doc attribute helper
// ---------------------------------------------------------------------------

fn doc_attr(desc: Option<&str>) -> TokenStream {
    match desc {
        Some(d) => quote! { #[doc = #d] },
        None => TokenStream::default(),
    }
}

// ---------------------------------------------------------------------------
// Pretty-printing
// ---------------------------------------------------------------------------

/// Attempt to pretty-print a `TokenStream` via `rustfmt`.
/// Falls back to `stream.to_string()` if `rustfmt` is unavailable.
fn prettyprint(stream: TokenStream) -> String {
    let raw = stream.to_string();
    // Try rustfmt
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new("rustfmt")
        .arg("--edition=2021")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return raw;
    };

    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        let _ = stdin.write_all(raw.as_bytes());
    }

    if let Ok(output) = child.wait_with_output() {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                return s;
            }
        }
    }

    raw
}
