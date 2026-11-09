//! Derive macros for Hydrargyros.
//!
//! [`FromVar`] turns a `Var` object into a typed struct, field by field —
//! the Rust-native answer to "script ergonomics, native speed": prototyping
//! code passes `Var` around, the hot path converts once and works with
//! plain structs.
//!
//! ```ignore
//! #[derive(FromVar)]
//! struct User {
//!     name: String,
//!     age: i64,
//!     #[from_var(rename = "userId")]
//!     id: String,
//!     #[from_var(default)]
//!     tags: Vec<String>,       // missing key → empty vec
//!     nickname: Option<String>, // missing key → None
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

/// Field options: `#[from_var(default)]` and `#[from_var(rename = "key")]`.
#[derive(Default)]
struct FieldOpts {
    default: bool,
    rename: Option<String>,
}

fn parse_opts(attr: &syn::Attribute, opts: &mut FieldOpts) -> syn::Result<()> {
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("default") {
            opts.default = true;
            Ok(())
        } else if meta.path.is_ident("rename") {
            let value: syn::LitStr = meta.value()?.parse()?;
            opts.rename = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("unknown from_var option (expected `default` or `rename = \"key\"`)"))
        }
    })
}

/// Derive `FromVar` for a named-field struct: `Var::from_var(&config)` reads
/// each field from the object by key (strict types — no silent coercion),
/// recording the field path in errors.
#[proc_macro_derive(FromVar, attributes(from_var))]
pub fn derive_from_var(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new(
                    input.ident.span(),
                    "FromVar only supports named-field structs",
                )
                .to_compile_error()
                .into()
            }
        },
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "FromVar only supports named-field structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut reads = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap();
        let ty = &field.ty;

        let mut opts = FieldOpts::default();
        let mut parse_error = None;
        for attr in &field.attrs {
            if attr.path().is_ident("from_var") {
                if let Err(e) = parse_opts(attr, &mut opts) {
                    parse_error = Some(e);
                }
            }
        }
        if let Some(e) = parse_error {
            return e.to_compile_error().into();
        }

        let key = opts.rename.unwrap_or_else(|| ident.to_string());
        let label = key.clone();

        let read = if opts.default {
            quote! {
                #ident: {
                    let __v = __root.get(#key);
                    if __v.ty() == ::hyg::Ty::Null {
                        ::std::default::Default::default()
                    } else {
                        <#ty as ::hyg::FromVar>::from_var(&__v)
                            .map_err(|e| e.at(#label))?
                    }
                }
            }
        } else {
            quote! {
                #ident: {
                    let __v = __root.get(#key);
                    <#ty as ::hyg::FromVar>::from_var(&__v)
                        .map_err(|e| e.at(#label))?
                }
            }
        };
        reads.push(read);
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        #[automatically_derived]
        impl #impl_generics ::hyg::FromVar for #name #ty_generics #where_clause {
            fn from_var(__root: &::hyg::Var) -> ::std::result::Result<Self, ::hyg::FromVarError> {
                if !matches!(__root.ty(), ::hyg::Ty::Object) {
                    return ::std::result::Result::Err(::hyg::FromVarError::new(
                        ::std::format!("expected object, found {}", __root.ty()),
                    ));
                }
                Ok(Self {
                    #(#reads,)*
                })
            }
        }
    }
    .into()
}
