//! Type-mapping helpers: CDPType → Rust TokenStream.

use heck::ToUpperCamelCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::pdl::parser::CdpType;

/// A CDP type resolved to its Rust token representation, including wrapping
/// metadata needed by [`crate::build::builder::StructBuilder`] to emit the
/// correct field type (e.g. `Option<Box<NodeId>>`, `Vec<String>`).
pub struct RustType {
    /// The base Rust type token stream (without `Option<>` or `Vec<>`).
    pub inner: TokenStream,
    /// True when the field must be wrapped in `Vec<_>`.
    pub is_vec: bool,
    /// True when the type should be boxed (e.g. recursive self-referential types).
    pub needs_box: bool,
}

impl RustType {
    pub fn simple(ts: TokenStream) -> Self {
        Self { inner: ts, is_vec: false, needs_box: false }
    }
    pub fn vec(ts: TokenStream) -> Self {
        Self { inner: ts, is_vec: true, needs_box: false }
    }
    pub fn boxed(ts: TokenStream) -> Self {
        Self { inner: ts, is_vec: false, needs_box: true }
    }

    /// Emit the full type expression (e.g. `Vec<String>`, `Box<NodeId>`, …).
    pub fn to_token_stream(&self) -> TokenStream {
        let inner = &self.inner;
        if self.is_vec {
            quote! { Vec<#inner> }
        } else if self.needs_box {
            quote! { Box<#inner> }
        } else {
            quote! { #inner }
        }
    }
}

/// Map a [`CdpType`] to a [`RustType`] for use in a struct or response type
/// that lives in `current_domain_mod`.
///
/// * `domain_name` — the snake_case name of the current domain module.
/// * `domain_pdl_index` — index of the current domain's PDL file (0 = browser,
///   1 = JS protocol, etc.).
/// * `domains` — map from domain name → pdl file index.
/// * `protocol_mods` — ordered list of PDL module names (e.g.
///   `["browser_protocol", "js_protocol"]`).
pub fn map_cdp_type(
    cdp_type: &CdpType,
    domain_name: &str,
    parent_name: &str,
    domain_pdl_index: usize,
    domains: &indexmap::IndexMap<String, usize>,
    protocol_mods: &[String],
) -> RustType {
    match cdp_type {
        CdpType::Integer => RustType::simple(quote! { i64 }),
        CdpType::Number => RustType::simple(quote! { f64 }),
        CdpType::Boolean => RustType::simple(quote! { bool }),
        CdpType::String => RustType::simple(quote! { String }),
        CdpType::Object | CdpType::Any => RustType::simple(quote! { serde_json::Value }),
        CdpType::Binary => RustType::simple(quote! { chromist_types::Binary }),
        CdpType::Array(inner) => {
            let inner_rt = map_cdp_type(
                inner,
                domain_name,
                parent_name,
                domain_pdl_index,
                domains,
                protocol_mods,
            );
            let inner_ts = inner_rt.inner.clone();
            // Arrays of refs don't need boxing inside Vec
            RustType::vec(inner_ts)
        }
        CdpType::Ref(name) => {
            // Detect self-referential types (boxed)
            let needs_box = name == parent_name;

            let ts = resolve_ref_type(name, domain_name, domain_pdl_index, domains, protocol_mods);
            if needs_box {
                RustType::boxed(ts)
            } else {
                RustType::simple(ts)
            }
        }
        // InlineEnum should be resolved to a Ref by resolve_inline_enum before
        // reaching map_cdp_type. This branch is a safety fallback.
        CdpType::InlineEnum(_) => RustType::simple(quote! { serde_json::Value }),
    }
}

/// Resolve a `Ref` type name to a Rust path token stream.
///
/// If `name` is `"Domain.TypeId"` we emit `super::domain_name::TypeId` (same
/// PDL file) or `super::super::protocol_mod::domain_name::TypeId` (different
/// PDL file).
///
/// If `name` is `"TypeId"` (no dot), it is same-domain and becomes just
/// `TypeId`.
pub fn resolve_ref_type(
    name: &str,
    current_domain: &str,
    current_pdl_index: usize,
    domains: &indexmap::IndexMap<String, usize>,
    protocol_mods: &[String],
) -> TokenStream {
    if let Some(dot) = name.find('.') {
        let ref_domain = &name[..dot];
        let type_name = &name[dot + 1..];
        let type_ident = format_ident!("{}", type_name.to_upper_camel_case());

        if ref_domain == current_domain {
            // Same domain — no module path needed
            quote! { #type_ident }
        } else {
            let ref_snake = ref_domain_snake(ref_domain);
            let domain_mod = format_ident!("{}", ref_snake);

            // Is the referenced domain in the same PDL file?
            let ref_pdl_index = domains.get(ref_domain).copied().unwrap_or(current_pdl_index);
            if ref_pdl_index == current_pdl_index {
                quote! { super::#domain_mod::#type_ident }
            } else {
                let proto_mod = format_ident!("{}", &protocol_mods[ref_pdl_index]);
                quote! { super::super::#proto_mod::#domain_mod::#type_ident }
            }
        }
    } else {
        // No dot: same-domain ref
        let type_ident = format_ident!("{}", name.to_upper_camel_case());
        quote! { #type_ident }
    }
}

/// Convert a CDP domain name (PascalCase) to its snake_case module name.
pub fn ref_domain_snake(domain: &str) -> String {
    use heck::ToSnakeCase;
    domain.to_snake_case()
}

/// Generate the string name for an inline sub-enum on a field.
/// e.g. parent type "ConsoleMessage", field "level" → "ConsoleMessageLevel"
pub fn subenum_name(parent_camel: &str, field_name: &str) -> String {
    let field_camel = field_name.to_upper_camel_case();
    format!("{parent_camel}{field_camel}")
}
