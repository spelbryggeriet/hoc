use proc_macro2::{Ident, Span, TokenStream};
use proc_macro_error::{abort, emit_warning, set_dummy};
use quote::quote;
use syn::{
    visit_mut::VisitMut, AttributeArgs, Item, Lit, Meta, MetaNameValue, NestedMeta, PathArguments,
    PathSegment, Token,
};

pub fn impl_define_commands(attrs: AttributeArgs, mut item: Item) -> TokenStream {
    set_dummy(quote!(#item));

    let name_values: Vec<_> = attrs
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

    struct MacroVisitor<'a> {
        name_values: Vec<(&'a Ident, String, bool)>,
    }

    impl VisitMut for MacroVisitor<'_> {
        fn visit_macro_mut(&mut self, m: &mut syn::Macro) {
            for (name, value, used) in self.name_values.iter_mut() {
                if m.path.is_ident(*name) {
                    m.path.leading_colon = Some(Token![::](Span::call_site()));
                    m.path.segments.insert(
                        0,
                        PathSegment {
                            ident: Ident::new("hoc_macros", Span::call_site()),
                            arguments: PathArguments::None,
                        },
                    );
                    m.path.segments[1].ident = Ident::new("cmd", Span::call_site());
                    let tokens = &m.tokens;
                    m.tokens = quote!(#value, #tokens);
                    *used = true;
                }
            }
        }
    }

    let mut visitor = MacroVisitor { name_values };
    visitor.visit_item_mut(&mut item);

    for (name, _, _) in visitor.name_values.iter().filter(|(_, _, used)| !used) {
        emit_warning!(name, "unused command");
    }

    quote!(#item)
}
