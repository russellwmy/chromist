//! Per-domain `Event` enum generation.
//!
//! Each domain that has events gets a `pub enum Event { … }` with one variant
//! per event struct, plus `impl TryFrom<CdpJsonEventMessage> for Event`.

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::pdl::parser::Event;

/// Generate the domain-level `Event` enum for one domain.
///
/// `domain_name` is the CamelCase domain name.
/// `events` is the list of events for that domain.
///
/// Returns a `TokenStream` containing the `Event` enum definition plus
/// a `Deserialize` impl based on the method identifier.
pub fn generate_domain_event_enum(domain_name: &str, events: &[Event]) -> TokenStream {
    if events.is_empty() {
        return TokenStream::default();
    }

    let variant_defs: Vec<TokenStream> = events
        .iter()
        .map(|ev| {
            let struct_name = format_ident!("{}Event", ev.name.to_upper_camel_case());
            let variant_name = format_ident!("{}", ev.name.to_upper_camel_case());
            let method_id = format!("{}.{}", domain_name, ev.name);
            quote! {
                #[serde(rename = #method_id)]
                #variant_name(#struct_name)
            }
        })
        .collect();

    quote! {
        /// All events for this domain.
        #[non_exhaustive]
        #[derive(Debug, Clone, serde::Deserialize)]
        #[serde(tag = "method", content = "params", rename_all = "camelCase")]
        pub enum Event {
            #(#variant_defs,)*
        }
    }
}

/// Generate a global `CdpEvent` enum across all domains and a matching
/// `CdpEventMessage` struct with a manual `Deserialize` impl.
///
/// `all_events` is a list of `(domain_name, snake_domain, event)` tuples,
/// ordered consistently with the generated domain modules.
pub fn generate_global_event_enum<'a>(
    all_events: &[(&'a str, &'a str, &'a Event)],
    protocol_mods: &[String],
    domains: &indexmap::IndexMap<String, usize>,
) -> TokenStream {
    if all_events.is_empty() {
        return TokenStream::default();
    }

    let mut variant_defs = TokenStream::default();
    let mut var_idents = Vec::new();
    let mut deser_arms = TokenStream::default();
    let mut identifier_arms = TokenStream::default();
    let mut conversion_impls = TokenStream::default();

    for (domain_name, _snake_domain, event) in all_events {
        let domain_camel = domain_name.to_upper_camel_case();
        let event_camel = event.name.to_upper_camel_case();
        let var_ident = format_ident!("{}{}", domain_camel, event_camel);
        let struct_name = format_ident!("{}Event", event_camel);
        let method_id = format!("{}.{}", domain_name, event.name);

        // Path to the type: e.g. `super::browser_protocol::page::LoadEventFiredEvent`
        let snake = domain_name.to_snake_case();
        let domain_mod = format_ident!("{}", snake);

        let current_pdl = domains.get(*domain_name).copied().unwrap_or(0);
        let proto_mod = format_ident!("{}", &protocol_mods[current_pdl]);

        let ty_path = quote! { super::#proto_mod::#domain_mod::#struct_name };

        variant_defs.extend(quote! {
            #var_ident(#ty_path),
        });

        deser_arms.extend(quote! {
            #method_id => CdpEvent::#var_ident(
                map.next_value::<#ty_path>()?
            ),
        });

        identifier_arms.extend(quote! {
            CdpEvent::#var_ident(inner) => inner.identifier(),
        });

        conversion_impls.extend(quote! {
            impl std::convert::TryFrom<CdpEvent> for #ty_path {
                type Error = CdpEvent;
                fn try_from(ev: CdpEvent) -> Result<Self, Self::Error> {
                    match ev {
                        CdpEvent::#var_ident(v) => Ok(v),
                        other => Err(other),
                    }
                }
            }
            impl From<#ty_path> for CdpEvent {
                fn from(v: #ty_path) -> Self {
                    CdpEvent::#var_ident(v)
                }
            }
        });

        var_idents.push(var_ident);
    }

    quote! {
        /// A typed CDP event decoded from a browser message.
        ///
        /// Every known event has a dedicated variant; events from protocol
        /// versions newer than the vendored PDL files are captured by the
        /// `Other` variant so they are never silently dropped.
        #[non_exhaustive]
        #[derive(Debug, Clone, PartialEq)]
        pub enum CdpEvent {
            #variant_defs
            /// A CDP event whose method name was not recognised by this
            /// version of the bindings.  The raw JSON params are preserved
            /// so callers can inspect or forward them.
            Other(serde_json::Value),
        }

        impl CdpEvent {
            /// Construct an `Other` variant wrapping a raw JSON value.
            pub fn other(val: serde_json::Value) -> Self {
                CdpEvent::Other(val)
            }
        }

        /// A fully-decoded inbound CDP event frame, combining the method
        /// identifier, optional session ID, and the typed event payload.
        #[derive(Debug, Clone, PartialEq)]
        pub struct CdpEventMessage {
            /// Fully-qualified CDP method name, e.g. `"Page.loadEventFired"`.
            pub method: chromist_types::MethodId,
            /// Session ID for multi-target sessions; `None` for browser-level events.
            pub session_id: Option<String>,
            /// Typed event payload.
            pub params: CdpEvent,
        }

        impl chromist_types::Method for CdpEventMessage {
            fn identifier(&self) -> chromist_types::MethodId {
                match &self.params {
                    #identifier_arms
                    _ => self.method.clone(),
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for CdpEventMessage {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de::{self, MapAccess, Visitor};
                use std::fmt;

                enum Field { Method, Session, Params }

                impl<'de> serde::Deserialize<'de> for Field {
                    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                        struct Vis;
                        impl<'de> Visitor<'de> for Vis {
                            type Value = Field;
                            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                                f.write_str("method, sessionId, or params")
                            }
                            fn visit_str<E: de::Error>(self, v: &str) -> Result<Field, E> {
                                match v {
                                    "method" => Ok(Field::Method),
                                    "sessionId" => Ok(Field::Session),
                                    "params" => Ok(Field::Params),
                                    other => Err(de::Error::unknown_field(other, &["method","sessionId","params"])),
                                }
                            }
                        }
                        d.deserialize_identifier(Vis)
                    }
                }

                struct MsgVisitor;
                impl<'de> Visitor<'de> for MsgVisitor {
                    type Value = CdpEventMessage;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("CdpEventMessage")
                    }
                    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        let mut method: Option<String> = None;
                        let mut session_id: Option<String> = None;
                        let mut params: Option<CdpEvent> = None;
                        while let Some(key) = map.next_key()? {
                            match key {
                                Field::Method => {
                                    if method.is_some() { return Err(de::Error::duplicate_field("method")); }
                                    method = Some(map.next_value()?);
                                }
                                Field::Session => {
                                    if session_id.is_some() { return Err(de::Error::duplicate_field("sessionId")); }
                                    session_id = Some(map.next_value()?);
                                }
                                Field::Params => {
                                    if params.is_some() { return Err(de::Error::duplicate_field("params")); }
                                    let m = method.as_deref().ok_or_else(|| de::Error::missing_field("method"))?;
                                    params = Some(match m {
                                        #deser_arms
                                        _ => {
                                    #[cfg(feature = "tracing")]
                                    ::tracing::warn!(method = m, "unrecognized CDP event; consider upgrading chromist-cdp");
                                    CdpEvent::Other(map.next_value()?)
                                }
                                    });
                                }
                            }
                        }
                        let method = method.ok_or_else(|| de::Error::missing_field("method"))?;
                        let params = params.ok_or_else(|| de::Error::missing_field("params"))?;
                        Ok(CdpEventMessage {
                            method: std::borrow::Cow::Owned(method),
                            session_id,
                            params,
                        })
                    }
                }
                deserializer.deserialize_struct("CdpEventMessage", &["method","sessionId","params"], MsgVisitor)
            }
        }

        #conversion_impls
    }
}
