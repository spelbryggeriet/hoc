use heck::ToKebabCase;
use proc_macro2::{Span, TokenStream};
use proc_macro_error::{abort, abort_call_site, set_dummy};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    DataStruct, DeriveInput, Ident, Token,
};

pub fn impl_procedure(input: DeriveInput) -> TokenStream {
    let command_fields: Vec<_> = match &input.data {
        syn::Data::Struct(DataStruct {
            fields: syn::Fields::Named(fields),
            ..
        }) => fields
            .named
            .iter()
            .map(|f| CommandField {
                ident: f.ident.as_ref().unwrap(),
                attrs: crate::parse_attributes("procedure", &f.attrs, &f.ident),
            })
            .collect(),
        _ => abort_call_site!("`Procedure` only supports structs with named fields"),
    };

    let command_name = &input.ident;
    let state_name = Ident::new(&format!("{command_name}State"), Span::call_site());
    let procedure_desc = command_name.to_string().to_kebab_case();

    set_dummy(quote! {
        impl ::hoc_core::procedure::Procedure for #command_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_desc;

            fn run(
                &mut self,
                _state: Self::State,
                _registry: &impl ::hoc_core::kv::WriteStore,
            ) -> ::hoc_log::Result<::hoc_core::procedure::Halt<Self::State>> {
                unreachable!()
            }
        }
    });

    let impl_procedure =
        gen_impl_procedure(command_name, &state_name, &procedure_desc, &command_fields);

    quote! {
        #impl_procedure
    }
}

fn gen_impl_procedure(
    struct_name: &Ident,
    state_name: &Ident,
    procedure_desc: &str,
    command_fields: &[CommandField],
) -> TokenStream {
    let get_attributes = gen_get_attributes(command_fields);
    let run = gen_run(command_fields);

    quote! {
        impl ::hoc_core::procedure::Procedure for #struct_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_desc;

            #get_attributes
            #run
        }
    }
}

fn gen_get_attributes(command_fields: &[CommandField]) -> TokenStream {
    let mut insertions = command_fields
        .iter()
        .filter(|f| {
            f.attrs
                .iter()
                .any(|a| matches!(a, CommandFieldAttr::Attribute))
        })
        .map(|f| {
            let title = crate::to_title_lower_case(f.ident.to_string());
            let ident = f.ident;
            quote! {
                attributes.insert(#title.to_string(), self.#ident.clone().to_string());
            }
        });

    let insertions = if let Some(insertion) = insertions.next() {
        Some(insertion).into_iter().chain(insertions)
    } else {
        return TokenStream::default();
    };

    quote! {
        fn get_attributes(&self) -> ::hoc_core::procedure::Attributes {
            let mut attributes = ::hoc_core::procedure::Attributes::new();
            #(#insertions;)*
            attributes
        }
    }
}

fn gen_run(command_fields: &[CommandField]) -> TokenStream {
    let defaults = command_fields
        .iter()
        .filter_map(|field| {
            field.attrs.iter().find_map(|attr| match attr {
                CommandFieldAttr::TryDefault(func) => Some((field.ident, func)),
                _ => None,
            })
        })
        .map(|(field, func)| {
            let prompt = format!(r#"Setting default "{}""#, field,);
            quote! {
                if self.#field.is_none() {
                    ::hoc_log::status!(#prompt => {
                        self.#field = Some(#func()?);
                    })
                }
            }
        });

    quote! {
        fn run(
            &mut self,
            state: Self::State,
            registry: &impl ::hoc_core::kv::WriteStore,
        ) -> ::hoc_log::Result<::hoc_core::procedure::Halt<Self::State>> {
            #(#defaults)*
            <Self::State as Run>::run(state, self, registry)
        }
    }
}

struct CommandField<'a> {
    ident: &'a Ident,
    attrs: Vec<CommandFieldAttr>,
}

#[derive(PartialOrd, Ord, Clone)]
enum CommandFieldAttr {
    Attribute,
    TryDefault(Ident),
    Rewind(Ident),
}

impl PartialEq for CommandFieldAttr {
    fn eq(&self, other: &Self) -> bool {
        use CommandFieldAttr::*;

        match (self, other) {
            (Attribute, Attribute) => true,
            (TryDefault(_), TryDefault(_)) => true,
            (Rewind(_), Rewind(_)) => true,
            _ => false,
        }
    }
}

impl Eq for CommandFieldAttr {}

impl Parse for CommandFieldAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        if input.peek(Token![=]) {
            let assign_token = input.parse::<Token![=]>()?;

            match input.parse::<Ident>() {
                Ok(ident) => match &*name_str {
                    "rewind" => Ok(Self::Rewind(ident)),
                    "try_default" => Ok(Self::TryDefault(ident)),
                    _ => abort!(name, "unexpected attribute: {}", name_str),
                },
                Err(_) => abort!(assign_token, "expected `identifier` after `=`"),
            }
        } else {
            match &*name_str {
                "attribute" => Ok(Self::Attribute),
                _ => abort!(name, "unexpected attribute: {}", name_str),
            }
        }
    }
}
