use proc_macro2::{Ident, TokenStream};
use proc_macro_error::{abort, emit_error, set_dummy};
use quote::quote_spanned;
use syn::{
    parse::Parser,
    punctuated::Punctuated,
    spanned::Spanned,
    visit_mut::{self, VisitMut},
    AttributeArgs, Expr, Item, Lit, Meta, MetaNameValue, NestedMeta, PathArguments, PathSegment,
    Token,
};

struct MacroVisitor<'a> {
    name_values: Vec<(&'a Ident, String, bool)>,
}

impl VisitMut for MacroVisitor<'_> {
    fn visit_macro_mut(&mut self, m: &mut syn::Macro) {
        for (i, (name, _, used)) in self.name_values.iter_mut().enumerate() {
            if let Some(old_ident) = m.path.get_ident().filter(|i| i == name) {
                let span = old_ident.span();
                m.path.leading_colon = Some(Token![::](span));
                m.path.segments.insert(
                    0,
                    PathSegment {
                        ident: Ident::new("hoc_macros", span),
                        arguments: PathArguments::None,
                    },
                );
                m.path.segments[1].ident = Ident::new("cmd", span);
                if let Ok(mut args) = Parser::parse(
                    Punctuated::<Expr, Token![,]>::parse_terminated,
                    m.tokens.clone().into(),
                ) {
                    *used = true;
                    for arg in args.iter_mut() {
                        visit_mut::visit_expr_mut(self, arg);
                    }
                    let value = &self.name_values[i].1;
                    m.tokens = quote_spanned!(m.tokens.span()=> #value, #args);
                    return;
                }
            }
        }
    }
}

pub fn impl_define_commands(args: AttributeArgs, mut item: Item) -> TokenStream {
    set_dummy(quote_spanned!(item.span()=> #item));

    let name_values: Vec<_> = args
        .iter()
        .map(|attr| match attr {
            NestedMeta::Meta(Meta::Path(path)) => {
                if path.get_ident().is_some() {
                    let name = &path.segments[0].ident;
                    let value = name.to_string();
                    (name, value, false)
                } else {
                    abort!(path, "expected single identifier")
                }
            }
            NestedMeta::Meta(Meta::NameValue(MetaNameValue { path, lit, .. })) => {
                if path.get_ident().is_some() {
                    if let Lit::Str(lit_str) = lit {
                        (&path.segments[0].ident, lit_str.value(), false)
                    } else {
                        abort!(lit, "expected string literal")
                    }
                } else {
                    abort!(path, "expected single identifier")
                }
            }
            _ => abort!(attr, "expected single identifier or name-value pair"),
        })
        .collect();

    let mut visitor = MacroVisitor { name_values };
    visitor.visit_item_mut(&mut item);

    for (name, _, _) in visitor.name_values.iter().filter(|(_, _, used)| !used) {
        emit_error!(name, "unused command");
    }

    quote_spanned!(item.span()=> #item)
}
