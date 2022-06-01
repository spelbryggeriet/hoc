use proc_macro2::{Ident, Span, TokenStream};
use proc_macro_error::{abort, abort_call_site};
use quote::quote;
use rand::Rng;
use syn::{
    punctuated::{IntoIter, Punctuated},
    Expr, ExprAssign, ExprLit, ExprPath, Lit, Token,
};

pub fn impl_cmd(args: Punctuated<Expr, Token![,]>) -> TokenStream {
    let mut args_iter = args.into_iter();
    let program = args_iter
        .next()
        .unwrap_or_else(|| abort_call_site!("requires at least a program name argument"));
    let args = if let Some(fmt) = args_iter.next() {
        gen_format_args(&fmt, args_iter)
    } else {
        quote!("")
    };

    quote! {
        ::hoc_core::process::Process::cmd(AsRef::<str>::as_ref(#program), #args)
    }
}

fn gen_format_args(fmt: &Expr, args_iter: IntoIter<Expr>) -> TokenStream {
    enum Arg {
        Positional(Expr),
        Assignment(Ident, Expr),
    }

    impl Arg {
        fn as_positional(&self) -> &Expr {
            match self {
                Self::Positional(expr) => expr,
                _ => panic!("expected positional argument"),
            }
        }
    }

    let fmt_str = match fmt {
        Expr::Lit(ExprLit {
            lit: Lit::Str(lit_str),
            ..
        }) => lit_str.value(),
        _ => abort!(fmt, "format argument must be a string literal"),
    };
    let mut fmt_str = fmt_str.as_str();

    let args: Vec<_> = args_iter
        .map(|arg| match arg {
            Expr::Assign(ExprAssign {
                attrs, left, right, ..
            }) if attrs.is_empty() => match *left {
                Expr::Path(ExprPath { attrs, path, .. })
                    if attrs.is_empty() && path.get_ident().is_some() =>
                {
                    Arg::Assignment(path.get_ident().unwrap().clone(), *right)
                }
                expr => Arg::Positional(expr),
            },
            expr => Arg::Positional(expr),
        })
        .collect();

    let positional_after_assignment = args
        .iter()
        .skip_while(|a| matches!(a, Arg::Positional(_)))
        .find_map(|a| match a {
            Arg::Positional(expr) => Some(expr),
            _ => None,
        });
    if let Some(expr) = positional_after_assignment {
        abort!(expr, "positional arguments cannot follow named arguments");
    }

    let assign_args_start = args
        .iter()
        .position(|a| matches!(a, Arg::Assignment(..)))
        .unwrap_or(args.len());
    let (position_args, assign_args) = args.split_at(assign_args_start);

    let mut args_index = 0;
    let mut args_quoted = Vec::new();
    let mut new_fmt = String::new();
    let mut rand_idents = Vec::new();
    let mut rng = rand::thread_rng();
    let mut used_indices = Vec::new();

    while let Some(brace_start) = fmt_str
        .char_indices()
        .find_map(|(i, c)| (c == '{').then(|| i))
    {
        new_fmt += &fmt_str[..brace_start];
        fmt_str = &fmt_str[brace_start..];
        let brace_count = fmt_str.chars().take_while(|c| *c == '{').count();
        new_fmt += &fmt_str[..brace_count];
        fmt_str = &fmt_str[brace_count..];

        if brace_count % 2 == 0 {
            continue;
        }

        let inside_brace;
        (inside_brace, fmt_str) = fmt_str.split_once('}').unwrap_or_else(||
            abort!(fmt, "invalid format string: expected `'}'` but string was terminated\nif you intended to print `{`, you can escape it using `{{`")
        );

        let (ident_str, quote_fmt) = if let Some((ident_str, rest)) = inside_brace.split_once(':') {
            (ident_str, format!("{{:{rest}}}"))
        } else {
            (inside_brace, "{}".to_string())
        };

        let mut new_ident: u32 = rng.gen();
        while rand_idents.contains(&new_ident) {
            new_ident = rng.gen();
        }
        rand_idents.push(new_ident);

        let new_ident_str = format!("__{new_ident}");
        let new_ident = Ident::new(&new_ident_str, Span::call_site());

        new_fmt += &new_ident_str;
        new_fmt += "}";

        let tokens = if ident_str.is_empty() {
            let expr = position_args
                .get(args_index)
                .unwrap_or_else(|| abort_call_site!("too few arguments were given"))
                .as_positional();
            used_indices.push(args_index);
            args_index += 1;
            quote! {
                #new_ident = ::hoc_core::process::Quotify::quotify(format!(#quote_fmt, #expr))
            }
        } else if let Ok(position) = ident_str.parse::<usize>() {
            let expr = position_args
                .get(position)
                .unwrap_or_else(|| abort_call_site!("too few arguments were given"))
                .as_positional();
            used_indices.push(position);
            quote! {
                #new_ident = ::hoc_core::process::Quotify::quotify(format!(#quote_fmt, #expr))
            }
        } else if let Some(expr) = assign_args.iter().find_map(|a| match a {
            Arg::Assignment(i, e) if i == ident_str => Some(e),
            _ => None,
        }) {
            quote! {
                #new_ident = ::hoc_core::process::Quotify::quotify(format!(#quote_fmt, #expr))
            }
        } else {
            let ident = Ident::new(ident_str, Span::call_site());
            quote! {
                #new_ident = ::hoc_core::process::Quotify::quotify(format!(#quote_fmt, #ident))
            }
        };

        args_quoted.push(tokens);
    }

    let too_many_args = (0..position_args.len()).any(|i| !used_indices.contains(&i));
    if too_many_args {
        abort_call_site!("too many arguments were given")
    }

    new_fmt += &fmt_str;

    quote!(format!(#new_fmt #(, #args_quoted)*))
}
