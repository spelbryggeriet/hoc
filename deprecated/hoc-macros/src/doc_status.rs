use std::mem;

use proc_macro2::TokenStream;
use proc_macro_error::{abort, set_dummy};
use quote::quote_spanned;
use syn::{
    parse::{ParseStream, Parser},
    parse_quote_spanned,
    spanned::Spanned,
    visit_mut::{self, VisitMut},
    Arm, Attribute, Expr, Item, LitStr, Stmt, Token,
};

struct DocVisitor;

impl DocVisitor {
    fn extract_statuses(&self, attrs: &mut Vec<Attribute>) -> Vec<Stmt> {
        let statuses = attrs
            .iter()
            .filter_map(|attr| {
                if attr.path.is_ident("doc") {
                    Parser::parse(
                        |input: ParseStream| {
                            let _eq: Token![=] = input.parse()?;
                            let status = input.parse::<LitStr>()?.value();
                            let stripped_status = status
                               .strip_prefix(char::is_whitespace)
                               .map(str::to_string)
                               .unwrap_or(status);
                            let escaped_status = stripped_status.replace("{", "{{").replace("}", "}}");
                            Ok(parse_quote_spanned!(attr.span()=> let __status = ::hoc_log::status!(#escaped_status);))
                        },
                        attr.tokens.clone().into(),
                    )
                    .ok()
                } else {
                    None
                }
            })
            .collect();

        *attrs = mem::take(attrs)
            .into_iter()
            .filter(|attr| !attr.path.is_ident("doc"))
            .collect();

        statuses
    }

    fn insert_statuses(&self, statuses: Vec<Stmt>, expr: &mut Expr) {
        if !statuses.is_empty() {
            match expr {
                Expr::Block(block) => {
                    block.block.stmts.splice(0..0, statuses);
                }
                _ => {
                    *expr = parse_quote_spanned!(expr.span()=> {
                        #(#statuses)*
                        #expr
                    });
                }
            }
        }
    }
}

impl VisitMut for DocVisitor {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let statuses = match expr {
            Expr::Binary(expr_binary) => {
                visit_mut::visit_expr_binary_mut(self, expr_binary);
                self.extract_statuses(&mut expr_binary.attrs)
            }
            Expr::Call(expr_call) => {
                visit_mut::visit_expr_call_mut(self, expr_call);
                self.extract_statuses(&mut expr_call.attrs)
            }
            Expr::MethodCall(expr_method_call) => {
                visit_mut::visit_expr_method_call_mut(self, expr_method_call);
                self.extract_statuses(&mut expr_method_call.attrs)
            }
            Expr::Macro(expr_macro) => {
                visit_mut::visit_expr_macro_mut(self, expr_macro);
                self.extract_statuses(&mut expr_macro.attrs)
            }
            Expr::Try(expr_try) => {
                visit_mut::visit_expr_try_mut(self, expr_try);
                self.extract_statuses(&mut expr_try.attrs)
            }
            Expr::Block(expr_block) => {
                visit_mut::visit_expr_block_mut(self, expr_block);
                self.extract_statuses(&mut expr_block.attrs)
            }
            _ => return visit_mut::visit_expr_mut(self, expr),
        };

        self.insert_statuses(statuses, expr);
    }

    fn visit_arm_mut(&mut self, arm: &mut Arm) {
        self.insert_statuses(self.extract_statuses(&mut arm.attrs), &mut arm.body);
        visit_mut::visit_arm_mut(self, arm);
    }

    fn visit_local_mut(&mut self, local: &mut syn::Local) {
        if let Some((_, init)) = &mut local.init {
            self.insert_statuses(self.extract_statuses(&mut local.attrs), init);
        }
        visit_mut::visit_local_mut(self, local);
    }
}

pub fn impl_doc_status(args: TokenStream, mut item: Item) -> TokenStream {
    set_dummy(quote_spanned!(item.span()=> #item));

    if !args.is_empty() {
        abort!(args, "`doc_status` does not take any arguments");
    }

    DocVisitor.visit_item_mut(&mut item);

    quote_spanned!(item.span()=> #item)
}
