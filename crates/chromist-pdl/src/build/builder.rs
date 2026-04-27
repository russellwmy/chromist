//! Builder struct code generation.
//!
//! For every struct with fields this emits:
//! - A `new(mandatory…)` constructor (when ≤ 4 mandatory fields).
//! - A `<Name>Builder` struct with a `builder()` factory, fluent setters,
//!   and a `build()` method.

use heck::ToSnakeCase;
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

/// A single CDP struct field, pre-resolved to its Rust representation, ready
/// for emission by [`StructBuilder`].
pub struct FieldSpec {
    /// Original CDP name (camelCase, used in `#[serde(rename = …)]`).
    pub cdp_name: String,
    /// Rust field name (snake_case, may be raw identifier like `r#type`).
    pub rust_name: Ident,
    /// The base Rust type token stream (not wrapped in `Option<>` or `Vec<>`).
    pub base_ty: TokenStream,
    /// True when the type should be wrapped in `Vec<_>`.
    pub is_vec: bool,
    /// True when the type should be `Box<_>`.
    pub needs_box: bool,
    /// True when the field is optional.
    pub optional: bool,
    /// Optional doc string.
    pub doc: Option<String>,
    /// Skip serde entirely (for manually added fields).
    pub serde_skip: bool,
    /// Extra serde attributes (e.g. `deserialize_with`).
    pub extra_serde: TokenStream,
}

impl FieldSpec {
    /// Emit the full field type (e.g. `Vec<String>`, `Option<Box<NodeId>>`).
    pub fn field_type_tokens(&self) -> TokenStream {
        let base = &self.base_ty;
        let inner = if self.is_vec {
            quote! { Vec<#base> }
        } else if self.needs_box {
            quote! { Box<#base> }
        } else {
            quote! { #base }
        };

        if self.optional {
            quote! { Option<#inner> }
        } else {
            inner
        }
    }

    /// Emit `pub field: Type` definition with doc + serde attributes.
    pub fn struct_field_tokens(&self) -> TokenStream {
        let name = &self.rust_name;
        let ty = self.field_type_tokens();
        let doc = doc_attr(self.doc.as_deref());
        let serde = self.serde_attr_tokens();
        quote! {
            #doc
            #serde
            pub #name: #ty
        }
    }

    fn serde_attr_tokens(&self) -> TokenStream {
        if self.serde_skip {
            return quote! { #[serde(skip)] };
        }
        let rename = &self.cdp_name;
        let mut attrs = quote! { #[serde(rename = #rename)] };

        if self.optional {
            attrs.extend(quote! {
                #[serde(skip_serializing_if = "Option::is_none")]
            });
        } else if self.is_vec {
            attrs.extend(quote! {
                #[serde(skip_serializing_if = "Vec::is_empty")]
            });
        }

        let extra = &self.extra_serde;
        if !extra.is_empty() {
            attrs.extend(quote! { #extra });
        }

        attrs
    }
}

fn doc_attr(desc: Option<&str>) -> TokenStream {
    match desc {
        Some(d) => quote! { #[doc = #d] },
        None => TokenStream::default(),
    }
}

/// Escape Rust reserved keywords for field names.
pub fn field_ident(name: &str) -> Ident {
    let snake = name.to_snake_case();
    match snake.as_str() {
        "type" => format_ident!("r#type"),
        "mod" => format_ident!("r#mod"),
        "override" => format_ident!("r#override"),
        "ref" => format_ident!("r#ref"),
        "use" => format_ident!("r#use"),
        "move" => format_ident!("r#move"),
        "loop" => format_ident!("r#loop"),
        "return" => format_ident!("r#return"),
        "self" => format_ident!("r#self"),
        "super" => format_ident!("r#super"),
        "crate" => format_ident!("r#crate"),
        "extern" => format_ident!("r#extern"),
        "in" => format_ident!("r#in"),
        "where" => format_ident!("r#where"),
        "as" => format_ident!("r#as"),
        "trait" => format_ident!("r#trait"),
        "impl" => format_ident!("r#impl"),
        "for" => format_ident!("r#for"),
        "match" => format_ident!("r#match"),
        "if" => format_ident!("r#if"),
        "else" => format_ident!("r#else"),
        "enum" => format_ident!("r#enum"),
        "struct" => format_ident!("r#struct"),
        "fn" => format_ident!("r#fn"),
        "let" => format_ident!("r#let"),
        "const" => format_ident!("r#const"),
        "static" => format_ident!("r#static"),
        "pub" => format_ident!("r#pub"),
        "async" => format_ident!("r#async"),
        "await" => format_ident!("r#await"),
        _ => format_ident!("{}", snake),
    }
}

/// Code generator for a single CDP struct: emits the struct definition, a
/// `new(mandatory…)` constructor when there are ≤ 4 mandatory fields, and a
/// `<Name>Builder` fluent builder for the remaining optional fields.
pub struct StructBuilder {
    /// PascalCase struct name (e.g. `NavigateParams`).
    pub name: Ident,
    /// All fields for this struct, in declaration order.
    pub fields: Vec<FieldSpec>,
}

impl StructBuilder {
    pub fn new(name: Ident) -> Self {
        Self { name, fields: vec![] }
    }

    /// Emit the struct definition.
    pub fn generate_struct_def(&self) -> TokenStream {
        let name = &self.name;
        let defs: Vec<_> = self.fields.iter().map(|f| f.struct_field_tokens()).collect();
        quote! {
            pub struct #name {
                #(#defs),*
            }
        }
    }

    /// Emit `new()`, `From`, and builder.
    pub fn generate_impl(&self) -> TokenStream {
        let mut stream = TokenStream::default();

        // new() constructor for ≤4 mandatory fields
        let mandatory: Vec<&FieldSpec> = self.fields.iter().filter(|f| !f.optional).collect();
        let optional_names: Vec<&Ident> =
            self.fields.iter().filter(|f| f.optional).map(|f| &f.rust_name).collect();

        if !mandatory.is_empty() && mandatory.len() <= 4 {
            let name = &self.name;
            let param_names: Vec<&Ident> = mandatory.iter().map(|f| &f.rust_name).collect();
            let param_tys: Vec<TokenStream> = mandatory
                .iter()
                .map(|f| {
                    if f.is_vec {
                        let base = &f.base_ty;
                        quote! { Vec<#base> }
                    } else {
                        let base = &f.base_ty;
                        quote! { impl Into<#base> }
                    }
                })
                .collect();
            let assigns: Vec<TokenStream> = mandatory
                .iter()
                .map(|f| {
                    let n = &f.rust_name;
                    if f.is_vec {
                        quote! { #n }
                    } else if f.needs_box {
                        quote! { #n: Box::new(#n.into()) }
                    } else {
                        quote! { #n: #n.into() }
                    }
                })
                .collect();

            stream.extend(quote! {
                impl #name {
                    pub fn new(#(#param_names: #param_tys),*) -> Self {
                        Self {
                            #(#assigns,)*
                            #(#optional_names: None,)*
                        }
                    }
                }
            });

            // impl From<String> when single String mandatory field
            if mandatory.len() == 1 {
                let f = mandatory[0];
                if !f.is_vec && f.base_ty.to_string() == "String" {
                    stream.extend(quote! {
                        impl<T: Into<String>> From<T> for #name {
                            fn from(v: T) -> Self {
                                #name::new(v)
                            }
                        }
                    });
                }
            }
        }

        // Builder
        stream.extend(self.generate_builder());

        stream
    }

    fn generate_builder(&self) -> TokenStream {
        let name = &self.name;
        let builder_name = format_ident!("{}Builder", name);

        let mandatory: Vec<&FieldSpec> = self.fields.iter().filter(|f| !f.optional).collect();

        // Builder field declarations (all Option<_>)
        let builder_fields: Vec<TokenStream> = self
            .fields
            .iter()
            .map(|f| {
                let n = &f.rust_name;
                let base = &f.base_ty;
                let bty = if f.is_vec {
                    quote! { Vec<#base> }
                } else {
                    quote! { #base }
                };
                quote! { pub #n: Option<#bty> }
            })
            .collect();

        // Setter methods
        let setters: Vec<TokenStream> = self
            .fields
            .iter()
            .map(|f| {
                let n = &f.rust_name;
                let base = &f.base_ty;
                if f.is_vec {
                    let singular_str = n.to_string();
                    let (iter_name, single_name) = if singular_str.ends_with('s') {
                        let sing = &singular_str[..singular_str.len() - 1];
                        (n.clone(), field_ident(sing))
                    } else {
                        (format_ident!("{}s", singular_str), n.clone())
                    };
                    quote! {
                        pub fn #single_name(mut self, val: impl Into<#base>) -> Self {
                            self.#n.get_or_insert_with(Vec::new).push(val.into());
                            self
                        }
                        pub fn #iter_name<I, S>(mut self, vals: I) -> Self
                        where
                            I: IntoIterator<Item = S>,
                            S: Into<#base>,
                        {
                            let v = self.#n.get_or_insert_with(Vec::new);
                            for val in vals { v.push(val.into()); }
                            self
                        }
                    }
                } else {
                    quote! {
                        pub fn #n(mut self, val: impl Into<#base>) -> Self {
                            self.#n = Some(val.into());
                            self
                        }
                    }
                }
            })
            .collect();

        // build() assignments
        let build_assigns: Vec<TokenStream> = self
            .fields
            .iter()
            .map(|f| {
                let n = &f.rust_name;
                let field_str = n.to_string();
                if f.optional {
                    if f.needs_box {
                        quote! { #n: self.#n.map(Box::new), }
                    } else {
                        quote! { #n: self.#n, }
                    }
                } else if f.needs_box {
                    quote! {
                        #n: Box::new(self.#n.ok_or(chromist_types::BuildError::new(#field_str))?),
                    }
                } else {
                    quote! {
                        #n: self.#n.ok_or(chromist_types::BuildError::new(#field_str))?,
                    }
                }
            })
            .collect();

        let build_fn = if mandatory.is_empty() {
            quote! {
                pub fn build(self) -> #name {
                    #name {
                        #(#build_assigns)*
                    }
                }
            }
        } else {
            quote! {
                pub fn build(self) -> Result<#name, chromist_types::BuildError> {
                    Ok(#name {
                        #(#build_assigns)*
                    })
                }
            }
        };

        quote! {
            impl #name {
                pub fn builder() -> #builder_name {
                    #builder_name::default()
                }
            }

            #[derive(Default, Clone)]
            pub struct #builder_name {
                #(#builder_fields,)*
            }

            impl #builder_name {
                #(#setters)*
                #build_fn
            }
        }
    }
}
