use heck::ToSnakeCase;
use proc_macro2::{Span, TokenStream};
use proc_macro_error::{abort, abort_call_site, set_dummy};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    DataEnum, DeriveInput, Fields, Ident, Type, Variant,
};

struct StateVariant<'a> {
    attrs: Vec<StateVariantAttr>,
    ident: &'a Ident,
    fields: Vec<StateVariantField<'a>>,
}

struct StateVariantField<'a> {
    #[allow(dead_code)]
    attrs: Vec<StateVariantFieldAttr>,
    ident: &'a Ident,
    ty: &'a Type,
}

#[derive(Debug, PartialOrd, Ord, Clone)]
enum StateVariantAttr {
    Transient,
    MaybeFinish,
    Finish,
}

impl PartialEq for StateVariantAttr {
    fn eq(&self, other: &Self) -> bool {
        use StateVariantAttr::*;

        match (self, other) {
            (Transient, Transient) => true,
            (MaybeFinish, MaybeFinish) => true,
            (Finish, Finish) => true,
            _ => false,
        }
    }
}

impl Eq for StateVariantAttr {}

impl Parse for StateVariantAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        match &*name_str {
            "transient" => Ok(Self::Transient),
            "maybe_finish" => Ok(Self::MaybeFinish),
            "finish" => Ok(Self::Finish),
            _ => abort!(name, "unexpected attribute: {}", name_str),
        }
    }
}

#[derive(PartialOrd, Ord, Clone)]
enum StateVariantFieldAttr {}

impl PartialEq for StateVariantFieldAttr {
    fn eq(&self, other: &Self) -> bool {
        #[allow(unused_imports)]
        use StateVariantFieldAttr::*;

        match (self, other) {
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }
}

impl Eq for StateVariantFieldAttr {}

impl Parse for StateVariantFieldAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        match &*name_str {
            _ => abort!(name, "unexpected attribute: {}", name_str),
        }
    }
}

pub fn impl_procedure_state(input: DeriveInput) -> proc_macro2::TokenStream {
    let state_variants: Vec<_> = match &input.data {
        syn::Data::Enum(DataEnum { variants, .. }) => {
            variants.iter().map(parse_state_variant).collect()
        }
        _ => abort_call_site!("`ProcedureState` only supports enums"),
    };

    let state_name = &input.ident;
    let state_name_str = state_name.to_string();
    let state_id_name = Ident::new(&format!("{state_name}Id"), Span::call_site());
    let command_name = Ident::new(
        state_name_str
            .strip_suffix("State")
            .unwrap_or(&state_name_str),
        Span::call_site(),
    );

    set_dummy(quote! {
        impl ::hoc_core::procedure::State for #state_name {
            type Id = #state_id_name;

            fn id(&self) -> Self::Id {
                unreachable!()
            }
        }

        impl ::hoc_core::procedure::Id for #state_id_name {
            type DeserializeError = ::strum::ParseError;

            fn description(&self) -> &'static str {
                unreachable!()
            }
        }

        impl Default for #state_name {
            fn default() -> Self {
                unreachable!()
            }
        }
    });

    let impl_state = gen_impl_state(state_name, &state_id_name);
    let impl_id = gen_impl_id(state_name, &state_id_name, &state_variants);
    let impl_default = gen_impl_default(state_name, &state_variants);

    let run_trait = gen_run_trait(&command_name, state_name, &state_variants);

    let gen = quote! {
        use #state_name::*;

        #impl_state
        #impl_id
        #impl_default

        #run_trait
    };

    gen.into()
}

fn gen_impl_state(state_name: &Ident, id_name: &Ident) -> TokenStream {
    quote! {
        impl ::hoc_core::procedure::State for #state_name {
            type Id = #id_name;

            fn id(&self) -> Self::Id {
                self.into()
            }
        }
    }
}

fn gen_impl_id(
    state_name: &Ident,
    id_name: &Ident,
    state_variants: &[StateVariant],
) -> TokenStream {
    let names: Vec<_> = state_variants.iter().map(|v| &v.ident).collect();
    let names_str: Vec<_> = names.iter().map(|n| n.to_string()).collect();
    let cases = state_variants.iter().map(|v| {
        let name = v.ident;
        let desc = crate::to_title_lower_case(v.ident.to_string());
        quote!(Self::#name => #desc,)
    });

    let match_switch = state_variants
        .is_empty()
        .then(|| quote!(unreachable!()))
        .or_else(|| Some(quote!(match self { #(#cases)* })));

    let id_name_str = id_name.to_string();
    let err_name = Ident::new(&format!("{id_name_str}FromStrErr"), Span::call_site());

    quote! {
        #[derive(Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub enum #id_name {
            #(#names,)*
        }

        #[derive(Debug)]
        pub struct #err_name(String);

        impl ::std::fmt::Display for #err_name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(f, "unknown ID: {}", self.0)
            }
        }

        impl ::std::error::Error for #err_name {}

        impl From<#id_name> for &'static str {
            fn from(id: #id_name) -> &'static str {
                #id_name_str
            }
        }

        impl From<&#state_name> for #id_name {
            fn from(state: &#state_name) -> Self {
                match state {
                    #(#state_name::#names { .. } => Self::#names,)*
                }
            }
        }

        impl ::std::str::FromStr for #id_name {
            type Err = #err_name;

            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                match s {
                    #(#names_str => Ok(Self::#names),)*
                    _ => Err(#err_name(s.to_string())),
                }
            }
        }

        impl ::hoc_core::procedure::Id for #id_name {
            type DeserializeError = #err_name;

            fn description(&self) -> &'static str {
                #match_switch
            }
        }
    }
}

fn gen_impl_default(state_name: &Ident, state_variants: &[StateVariant]) -> TokenStream {
    let default_state_variant = state_variants.get(0).map_or_else(
        || quote!(unreachable!()),
        |v| {
            let name = v.ident;
            let fields = v.fields.iter().map(|f| {
                let field_name = &f.ident;
                quote!(#field_name: Default::default())
            });
            quote!(Self::#name { #(#fields),* })
        },
    );

    quote! {
        impl Default for #state_name {
            fn default() -> Self {
                #default_state_variant
            }
        }
    }
}

fn gen_run_trait(
    command_name: &Ident,
    state_name: &Ident,
    state_variants: &[StateVariant],
) -> TokenStream {
    let run_fns = state_variants.iter().map(|v| {
        let name = Ident::new(&v.ident.to_string().to_snake_case(), Span::call_site());
        let args = v.fields.iter().map(|f| {
            let field_name = f.ident;
            let field_type = f.ty;
            quote!(#field_name: #field_type)
        });

        let proc_registry_type = if v.attrs.contains(&StateVariantAttr::Transient) {
            quote!(ReadStore)
        } else {
            quote!(WriteStore)
        };

        let return_type = if v.attrs.contains(&StateVariantAttr::Finish) {
            quote!(())
        } else if v.attrs.contains(&StateVariantAttr::MaybeFinish) {
            quote!(Option<Self>)
        } else {
            quote!(Self)
        };

        quote! {
            fn #name(
                procedure: &mut #command_name,
                proc_registry: &impl #proc_registry_type,
                global_registry: &impl ::hoc_core::kv::ReadStore
                #(, #args)*
            ) -> ::hoc_log::Result<#return_type>;
        }
    });

    let maybe_impl_run = state_variants
        .is_empty()
        .then(|| quote!(impl Run for #state_name {}));

    let variant_patterns = state_variants.iter().map(|v| {
        let variant_name = v.ident;
        let field_names = v.fields.iter().map(|f| &f.ident);

        quote!(#variant_name { #(#field_names),* })
    });

    let variant_exprs = state_variants.iter().map(|v| {
        let name = Ident::new(&v.ident.to_string().to_snake_case(), Span::call_site());
        let args = v.fields.iter().map(|f| &f.ident);
        let persist = !v.attrs.contains(&StateVariantAttr::Transient);

        if v.attrs.contains(&StateVariantAttr::Finish) {
            quote!({
                #state_name::#name(procedure, proc_registry, global_registry #(, #args)*)?;
                ::hoc_core::procedure::Halt {
                    persist: #persist,
                    state: ::hoc_core::procedure::HaltState::Finish,
                }
            })
        } else if v.attrs.contains(&StateVariantAttr::MaybeFinish) {
            quote!({
                let new_state = #state_name::#name(procedure, proc_registry, global_registry #(, #args)*)?;
                ::hoc_core::procedure::Halt {
                    persist: #persist,
                    state: new_state
                        .map(::hoc_core::procedure::HaltState::Halt)
                        .unwrap_or(::hoc_core::procedure::HaltState::Finish),
                }
            })
        } else {
            quote!({
                let new_state = #state_name::#name(procedure, proc_registry, global_registry #(, #args)*)?;
                ::hoc_core::procedure::Halt {
                    persist: #persist,
                    state: ::hoc_core::procedure::HaltState::Halt(new_state),
                }
            })
        }
    });

    let match_switch = state_variants
        .is_empty()
        .then(|| quote!(unreachable!()))
        .or_else(|| {
            Some(quote!(match state {
                #(#variant_patterns => #variant_exprs,)*
            }))
        });

    quote! {
        trait RunImplRequired: Run {}

        impl RunImplRequired for #state_name {}
        #maybe_impl_run

        trait Run: Sized {
            fn run(
                state: #state_name,
                procedure: &mut #command_name,
                proc_registry: &impl ::hoc_core::kv::WriteStore,
                global_registry: &impl ::hoc_core::kv::ReadStore,
            ) -> ::hoc_log::Result<::hoc_core::procedure::Halt<#state_name>> {
                let halt = #match_switch;
                Ok(halt)
            }

            #(#run_fns)*
        }
    }
}

fn parse_state_variant(variant: &Variant) -> StateVariant {
    match variant {
        Variant {
            attrs,
            ident,
            fields: Fields::Named(ref fields),
            discriminant: None,
        } => StateVariant {
            attrs: crate::parse_attributes("state", attrs, ident),
            ident,
            fields: fields
                .named
                .iter()
                .map(|f| {
                    let ident = f.ident.as_ref().unwrap();
                    StateVariantField {
                        attrs: crate::parse_attributes("state", &f.attrs, ident),
                        ident,
                        ty: &f.ty,
                    }
                })
                .collect(),
        },
        Variant {
            attrs,
            ident,
            fields: Fields::Unit,
            discriminant: None,
        } => StateVariant {
            attrs: crate::parse_attributes("state", attrs, ident),
            ident,
            fields: Vec::new(),
        },
        Variant {
            discriminant: Some((_eq, ref dis)),
            ..
        } => abort!(dis, "discriminants not supported"),
        _ => abort!(variant, "`ProcedureState` only supports non-tuple variants"),
    }
}
