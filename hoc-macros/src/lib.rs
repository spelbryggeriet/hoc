use heck::ToTitleCase;
use proc_macro_error::{abort, proc_macro_error, ResultExt};
use quote::ToTokens;
use syn::{parse::Parse, parse_macro_input, punctuated::Punctuated, Attribute, Token};

mod procedure;
mod procedure_state;

#[proc_macro_error]
#[proc_macro_derive(Procedure, attributes(procedure))]
pub fn procedure(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    procedure::impl_procedure(parse_macro_input!(item)).into()
}

#[proc_macro_error]
#[proc_macro_derive(ProcedureState, attributes(state))]
pub fn procedure_state(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    procedure_state::impl_procedure_state(parse_macro_input!(item)).into()
}

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

fn parse_attributes<T: Parse + Clone + Ord, U: ToTokens>(
    attr_name: &str,
    attrs: &[Attribute],
    blame_tokens: U,
) -> Vec<T> {
    let parsed: Vec<T> = attrs
        .iter()
        .filter(|a| a.path.is_ident(attr_name))
        .flat_map(|a| {
            a.parse_args_with(Punctuated::<T, Token![,]>::parse_terminated)
                .unwrap_or_abort()
        })
        .collect();
    let mut sorted_parsed = parsed.clone();
    sorted_parsed.sort();
    sorted_parsed.dedup_by(|a, b| {
        if a == b {
            abort!(blame_tokens, "duplicate attributes specified");
        } else {
            false
        }
    });
    parsed
}
