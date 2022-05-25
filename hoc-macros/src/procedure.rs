use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Span, TokenStream};
use proc_macro_error::{abort, abort_call_site, set_dummy};
use quote::quote;
use syn::{
    parenthesized,
    parse::{Parse, ParseStream},
    DataStruct, DeriveInput, Ident, Token, Type, TypePath,
};

pub fn impl_procedure(input: DeriveInput) -> TokenStream {
    let command_attributes = crate::parse_attributes("procedure", &input.attrs, &input.ident);
    let command_fields: Vec<_> = match &input.data {
        syn::Data::Struct(DataStruct {
            fields: syn::Fields::Named(fields),
            ..
        }) => fields
            .named
            .iter()
            .map(|f| CommandField {
                ident: f.ident.as_ref().unwrap(),
                ty: &f.ty,
                attrs: crate::parse_attributes("procedure", &f.attrs, &f.ident),
            })
            .collect(),
        _ => abort_call_site!("`Procedure` only supports structs with named fields"),
    };

    let command_name = &input.ident;
    let state_name = Ident::new(&format!("{command_name}State"), Span::call_site());
    let procedure_name = command_name.to_string().to_upper_camel_case();

    set_dummy(quote! {
        impl ::hoc_core::procedure::Procedure for #command_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_name;

            fn run(
                &mut self,
                _state: Self::State,
                _registry: &impl ::hoc_core::kv::WriteStore,
            ) -> ::hoc_log::Result<::hoc_core::procedure::Halt<Self::State>> {
                unreachable!()
            }
        }
    });

    let impl_procedure = gen_impl_procedure(
        command_name,
        &state_name,
        &procedure_name,
        &command_attributes,
        &command_fields,
    );

    quote! {
        #impl_procedure
    }
}

fn gen_impl_procedure(
    command_name: &Ident,
    state_name: &Ident,
    procedure_name: &str,
    command_attributes: &[CommandAttr],
    command_fields: &[CommandField],
) -> TokenStream {
    let get_attributes = gen_get_attributes(command_fields);
    let get_dependencies = gen_get_dependencies(command_name, command_attributes, command_fields);
    let run = gen_run(command_fields);

    quote! {
        impl ::hoc_core::procedure::Procedure for #command_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_name;

            #get_attributes
            #get_dependencies
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
            let name = f.ident.to_string().to_snake_case();
            let ident = f.ident;
            quote! {
                attributes.insert(#name.to_string(), self.#ident.clone().to_string());
            }
        });

    let insertions = if let Some(insertion) = insertions.next() {
        Some(insertion).into_iter().chain(insertions)
    } else {
        return TokenStream::default();
    };

    quote! {
        fn get_attributes(&self) -> ::hoc_core::procedure::Attributes {
            let mut attributes = ::hoc_core::procedure::Attributes::default();
            #(#insertions)*
            attributes
        }
    }
}

fn gen_get_dependencies(
    command_name: &Ident,
    command_attributes: &[CommandAttr],
    command_fields: &[CommandField],
) -> TokenStream {
    let dependencies = command_attributes.iter().find_map(|attr| {
        #[allow(irrefutable_let_patterns)]
        if let CommandAttr::Dependencies(deps) = attr {
            Some(deps)
        } else {
            None
        }
    });

    let dependencies = if let Some(dependencies) = dependencies {
        dependencies
    } else {
        return TokenStream::default();
    };

    let insertions = dependencies.iter().map(|dep| {
        let name = dep.procedure.to_string();
        let attr_insertions = dep.attributes.iter().map(|(name, field)| {
            let name = name.to_string();
            if let Some(command_field) = command_fields.iter().find(|f| f.ident == field) {
                if let Type::Path(TypePath { path, .. }) = command_field.ty {
                    if path
                        .segments
                        .last()
                        .map_or(false, |seg| seg.ident == "Option")
                    {
                        return quote! {
                            if let Some(field) = self.#field.as_ref() {
                                attributes.insert(#name.to_string(), field.to_string());
                            }
                        };
                    }
                }

                quote! {
                    attributes.insert(#name.to_string(), self.#field.to_string());
                }
            } else {
                abort!(field, "no field `{}` in `{}`", field, command_name)
            }
        });

        quote! {
            dependencies.insert(::hoc_core::procedure::Key::new(
                #name.to_string(),
                {
                    let mut attributes = ::hoc_core::procedure::Attributes::default();
                    #(#attr_insertions)*
                    attributes
                }
            ));
        }
    });

    quote! {
        fn get_dependencies(&self) -> ::hoc_core::procedure::Dependencies {
            let mut dependencies = ::hoc_core::procedure::Dependencies::default();
            #(#insertions)*
            dependencies
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
    ty: &'a Type,
    attrs: Vec<CommandFieldAttr>,
}

#[derive(Clone)]
enum CommandAttr {
    Dependencies(Vec<Dependency>),
}

impl PartialEq for CommandAttr {
    fn eq(&self, other: &Self) -> bool {
        use CommandAttr::*;

        #[allow(unreachable_patterns)]
        match (self, other) {
            (Dependencies(_), Dependencies(_)) => true,
            _ => false,
        }
    }
}

impl Eq for CommandAttr {}

impl Parse for CommandAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        match &*name_str {
            "dependencies" => {
                let content;
                let _paren = parenthesized!(content in input);
                let deps_punct = content.parse_terminated::<Dependency, Token![,]>(Parse::parse)?;

                let mut dependencies = Vec::<Dependency>::with_capacity(deps_punct.len());
                for dep in deps_punct.into_iter() {
                    if dependencies.iter().any(|d| d.procedure == dep.procedure) {
                        abort!(dep.procedure, "duplicate procedure specified");
                    } else {
                        dependencies.push(dep);
                    }
                }

                Ok(Self::Dependencies(dependencies))
            }
            _ => abort!(name, "unexpected attribute: {}", name_str),
        }
    }
}

#[derive(Clone)]
struct Dependency {
    procedure: Ident,
    attributes: Vec<(Ident, Ident)>,
}

impl Parse for Dependency {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let procedure: Ident = input.parse()?;

        let content;
        let _paren = parenthesized!(content in input);

        let attrs_punct = content.parse_terminated::<_, Token![,]>(|content_input| {
            let name: Ident = content_input.parse()?;
            let _eq: Token![=] = content_input.parse()?;
            let attr: Ident = content_input.parse()?;
            Ok((name, attr))
        })?;

        let mut attributes = Vec::<(Ident, Ident)>::with_capacity(attrs_punct.len());
        for attr in attrs_punct.into_iter() {
            if attributes.iter().any(|(n, _)| n == &attr.0) {
                abort!(attr.0, "duplicate procedure attribute specified");
            } else {
                attributes.push(attr);
            }
        }

        Ok(Self {
            procedure,
            attributes,
        })
    }
}

#[derive(Clone)]
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
