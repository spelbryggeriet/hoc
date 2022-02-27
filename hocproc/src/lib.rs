use heck::{ToKebabCase, ToSnakeCase, ToTitleCase};
use proc_macro2::{Span, TokenStream};
use proc_macro_error::{abort, proc_macro_error, set_dummy, ResultExt};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    *,
};

fn to_title_lower_case<S: AsRef<str>>(s: S) -> String {
    let uppercase_title = s.as_ref().to_title_case();
    let mut title = String::with_capacity(uppercase_title.capacity());
    let mut iter = uppercase_title.split(' ');
    title += iter.next().unwrap_or_default();
    for word in iter {
        title += " ";
        title += &word.to_lowercase();
    }
    title
}

struct CommandField<'a> {
    ident: &'a Ident,
    attrs: Vec<CommandAttr>,
}

struct StateVariant<'a> {
    ident: &'a Ident,
    fields: Vec<(&'a Ident, &'a Type)>,
    unit: bool,
}

enum CommandAttr {
    Attribute,
    Rewind(Ident),
}

impl Parse for CommandAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        if input.peek(Token![=]) {
            let assign_token = input.parse::<Token![=]>()?;

            match input.parse::<Ident>() {
                Ok(ident) => match &*name_str {
                    "rewind" => Ok(Self::Rewind(ident)),
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

struct ProcedureTypes {
    command: ItemStruct,
    state: ItemEnum,
}

impl Parse for ProcedureTypes {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Self {
            command: input.parse()?,
            state: input.parse()?,
        })
    }
}

#[proc_macro_error]
#[proc_macro]
pub fn procedure(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let types = parse_macro_input!(item as ProcedureTypes);
    let gen = impl_procedure(&types);
    gen.into()
}

fn parse_command_attributes(attrs: &[Attribute]) -> Vec<CommandAttr> {
    attrs
        .iter()
        .filter(|a| a.path.is_ident("procedure"))
        .flat_map(|a| {
            a.parse_args_with(Punctuated::<CommandAttr, Token![,]>::parse_terminated)
                .unwrap_or_abort()
        })
        .collect()
}

fn parse_state_variant(variant: &Variant) -> StateVariant {
    match variant {
        Variant {
            discriminant: None,
            fields: Fields::Named(ref fields),
            ..
        } => StateVariant {
            ident: &variant.ident,
            fields: fields
                .named
                .iter()
                .map(|f| (f.ident.as_ref().unwrap(), &f.ty))
                .collect(),
            unit: false,
        },
        Variant {
            discriminant: None,
            fields: Fields::Unit,
            ..
        } => StateVariant {
            ident: &variant.ident,
            fields: Vec::new(),
            unit: true,
        },
        Variant {
            discriminant: Some((_eq, ref dis)),
            ..
        } => abort!(dis, "discriminants not supported"),
        _ => abort!(
            variant,
            "procedure only supports non-tuple variants as state"
        ),
    }
}

fn impl_procedure(types: &ProcedureTypes) -> TokenStream {
    let mut stripped_command = types.command.clone();
    let command = &types.command;
    let state = &types.state;

    let command_name = &command.ident;
    let state_name = &state.ident;
    let state_id_name = Ident::new(&format!("{}Id", state_name), Span::call_site());
    let procedure_desc = command_name.to_string().to_kebab_case();

    for field in stripped_command.fields.iter_mut() {
        field.attrs.retain(|a| !a.path.is_ident("procedure"));
    }

    set_dummy(quote! {
        use #state_name::*;

        #[derive(::structopt::StructOpt)]
        #stripped_command

        impl ::hoclib::Procedure for #command_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_desc;

            fn run(&mut self, _step: &mut ::hoclib::ProcedureStep) -> ::hoclog::Result<::hoclib::Halt<Self::State>> {
                unreachable!()
            }
        }

        #[derive(Debug, ::serde::Serialize, ::serde::Deserialize, ::strum::EnumDiscriminants)]
        #[strum_discriminants(derive(Hash, PartialOrd, Ord, ::strum::EnumString, ::strum::IntoStaticStr))]
        #[strum_discriminants(name(#state_id_name))]
        #state

        impl ::hoclib::ProcedureState for #state_name {
            type Id = #state_id_name;

            fn id(&self) -> Self::Id {
                unreachable!()
            }
        }

        impl ::hoclib::ProcedureStateId for #state_id_name {
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

    let state_variants: Vec<_> = types
        .state
        .variants
        .iter()
        .map(parse_state_variant)
        .collect();

    let impl_procedure = gen_impl_procedure(
        command_name,
        state_name,
        &state_id_name,
        &procedure_desc,
        &types.command,
        &state_variants,
    );
    let impl_procedure_state = gen_impl_procedure_state(state_name, &state_id_name);
    let impl_procedure_state_id = gen_impl_procedure_state_id(&state_id_name, &state_variants);
    let impl_default = gen_impl_default(state_name, &state_variants);

    let steps_trait = gen_steps_trait(command_name, state_name, &state_variants);

    quote! {
        use #state_name::*;

        #[derive(::structopt::StructOpt)]
        #stripped_command

        #impl_procedure

        #[derive(Debug, ::serde::Serialize, ::serde::Deserialize, ::strum::EnumDiscriminants)]
        #[strum_discriminants(derive(Hash, PartialOrd, Ord, ::strum::EnumString, ::strum::IntoStaticStr))]
        #[strum_discriminants(name(#state_id_name))]
        #state

        #impl_procedure_state
        #impl_procedure_state_id
        #impl_default

        #steps_trait
    }
}

fn gen_impl_procedure(
    struct_name: &Ident,
    state_name: &Ident,
    state_id_name: &Ident,
    procedure_desc: &str,
    command: &ItemStruct,
    state_variants: &[StateVariant],
) -> TokenStream {
    let command_fields: Vec<_> = match &command.fields {
        Fields::Named(ref fields) => fields
            .named
            .iter()
            .map(|f| CommandField {
                ident: f.ident.as_ref().unwrap(),
                attrs: parse_command_attributes(&f.attrs),
            })
            .collect(),
        _ => abort!(
            command,
            "procedure only supports non-tuple structs as commands"
        ),
    };

    let get_attributes = gen_get_attributes(&command_fields);
    let rewind_state = gen_rewind_state(&state_id_name, &command_fields, state_variants);
    let run = gen_run(state_name, &state_variants);

    quote! {
        impl ::hoclib::Procedure for #struct_name {
            type State = #state_name;
            const NAME: &'static str = #procedure_desc;

            #get_attributes
            #rewind_state
            #run
        }
    }
}

fn gen_impl_procedure_state(state_name: &Ident, state_id_name: &Ident) -> TokenStream {
    quote! {
        impl ::hoclib::ProcedureState for #state_name {
            type Id = #state_id_name;

            fn id(&self) -> Self::Id {
                self.into()
            }
        }
    }
}

fn gen_impl_procedure_state_id(
    state_id_name: &Ident,
    state_variants: &[StateVariant],
) -> TokenStream {
    let cases = state_variants.iter().map(|v| {
        let name = v.ident;
        let desc = to_title_lower_case(v.ident.to_string());
        quote!(Self::#name => #desc,)
    });

    let match_switch = state_variants
        .is_empty()
        .then(|| quote!(unreachable!()))
        .or_else(|| Some(quote!(match self { #(#cases)* })));

    quote! {
        impl ::hoclib::ProcedureStateId for #state_id_name {
            type DeserializeError = ::strum::ParseError;

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
            if v.unit {
                quote!(Self::#name)
            } else {
                let fields = v.fields.iter().map(|f| {
                    let field_name = &f.0;
                    quote!(#field_name: Default::default())
                });
                quote!({ #(#fields),* })
            }
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

fn gen_get_attributes(command_fields: &[CommandField]) -> TokenStream {
    let mut insertions = command_fields
        .iter()
        .filter(|f| f.attrs.iter().any(|a| matches!(a, CommandAttr::Attribute)))
        .map(|f| {
            let title = to_title_lower_case(f.ident.to_string());
            let ident = f.ident;
            quote!(variant.insert(#title.to_string(), self.#ident.clone().into());)
        });

    let insertions = if let Some(insertion) = insertions.next() {
        Some(insertion).into_iter().chain(insertions)
    } else {
        return TokenStream::default();
    };

    quote! {
        fn get_attributes(&self) -> ::hoclib::Attributes {
            let mut variant = ::hoclib::Attributes::new();
            #(#insertions)*
            variant
        }
    }
}

fn gen_rewind_state(
    state_id_name: &Ident,
    command_fields: &[CommandField],
    state_variants: &[StateVariant],
) -> TokenStream {
    let mut rewinds: Vec<_> = command_fields
        .iter()
        .filter_map(|f| {
            let rewind = f.attrs.iter().find_map(|a| {
                if let CommandAttr::Rewind(rewind) = a {
                    Some(rewind)
                } else {
                    None
                }
            })?;
            let name = f.ident;
            Some((rewind, name))
        })
        .collect();

    if rewinds.is_empty() {
        return TokenStream::default();
    }

    let state_id_order: Vec<_> = state_variants.iter().map(|v| v.ident).collect();

    for (rewind, _) in &rewinds {
        if !state_id_order.contains(rewind) {
            abort!(rewind, "`{}` is not a valid state ID", rewind);
        }
    }

    rewinds.sort_by_key(|(r, _)| state_id_order.iter().position(|i| i == r).unwrap());

    let mut rewinds = rewinds
        .into_iter()
        .map(|(rewind, name)| quote!(self.#name.then(|| #state_id_name::#rewind)));
    let first = rewinds.next().unwrap();

    quote! {
        fn rewind_state(&self) -> Option<<Self::State as ::hoclib::ProcedureState>::Id> {
            #first #(.or_else(|| #rewinds))*
        }
    }
}

fn gen_run(state_name: &Ident, state_variants: &[StateVariant]) -> TokenStream {
    let variant_patterns = state_variants.iter().map(|v| {
        let variant_name = v.ident;
        let field_names = v.fields.iter().map(|f| &f.0);

        if v.unit {
            quote!(#state_name::#variant_name)
        } else {
            quote!(#state_name::#variant_name { #(#field_names),* })
        }
    });

    let calls = state_variants.iter().map(|v| {
        let name = Ident::new(&v.ident.to_string().to_snake_case(), Span::call_site());
        let args = v.fields.iter().map(|f| &f.0);
        quote!(self.#name(step.work_dir_state_mut() #(, #args)*))
    });

    let match_switch = state_variants
        .is_empty()
        .then(|| quote!(unreachable!()))
        .or_else(|| Some(quote!(match step.state()? { #(#variant_patterns => #calls,)* })));

    quote! {
        #[allow(unreachable_code)]
        fn run(&mut self, step: &mut ::hoclib::ProcedureStep) -> ::hoclog::Result<::hoclib::Halt<Self::State>> {
            #match_switch
        }
    }
}

fn gen_steps_trait(
    command_name: &Ident,
    state_name: &Ident,
    state_variants: &[StateVariant],
) -> TokenStream {
    let step_fns = state_variants.iter().map(|v| {
        let name = Ident::new(&v.ident.to_string().to_snake_case(), Span::call_site());
        let args = v.fields.iter().map(|f| {
            let field_name = f.0;
            let field_type = f.1;
            quote!(#field_name: #field_type)
        });

        quote!(fn #name(&mut self, work_dir_state: &mut ::hoclib::DirState #(, #args)*) -> ::hoclog::Result<::hoclib::Halt<#state_name>>;)
    });

    let maybe_impl_steps = state_variants
        .is_empty()
        .then(|| quote!(impl Steps for #command_name {}));

    quote! {
        trait StepsImplRequired: Steps {}

        impl StepsImplRequired for #command_name {}
        #maybe_impl_steps

        trait Steps {
            #(#step_fns)*
        }
    }
}
