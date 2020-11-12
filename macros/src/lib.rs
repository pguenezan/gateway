use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, spanned::Spanned, Expr, ExprLit, Lit, Member};

fn expr_to_str(expr: Expr) -> String {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(string),
            ..
        }) => string.value(),
        expr => {
            let mut tokens = TokenStream::new();
            expr.to_tokens(&mut tokens);
            format!("{}", tokens)
        }
    }
}

macro_rules! to_compile_error {
    ($span:expr, $msg:expr) => {
        proc_macro::TokenStream::from(syn::Error::new($span, $msg).to_compile_error())
    };
}

#[proc_macro]
pub fn gateway_config(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as Expr);

    let array = match input {
        Expr::Array(array) => array,
        _ => return to_compile_error!(input.span(), "config should be an array"),
    };

    let mut cases = TokenStream::new();

    for elem in array.elems {
        let structure = match elem {
            Expr::Struct(structure) => structure,
            _ => return to_compile_error!(elem.span(), "not a structure"),
        };
        if structure.path.segments.len() != 1 {
            return to_compile_error!(structure.path.span(), "a single path segment is expected");
        }
        if structure.path.segments[0].ident != "Api" {
            return to_compile_error!(structure.path.span(), "structure must be of type `Api`");
        }

        let mut content = HashMap::new();
        for field in structure.fields {
            match field.member {
                Member::Named(ident) => {
                    content.insert(ident.to_string(), expr_to_str(field.expr));
                }
                _ => return to_compile_error!(field.span(), "field should be named"),
            }
        }

        // TODO check content

        let app_name = content.get("app_name");
        let host = content.get("host");

        cases.extend(quote! {
            #app_name => {
                let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_path);
                match uri_string.parse() {
                    Ok(uri) => *req.uri_mut() = uri,
                    Err(_) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
                };
                match client.request(req).await {
                    Ok(response) => Ok(response),
                    Err(_) => get_response!(StatusCode::BAD_GATEWAY, BADGATEWAY),
                }
            },
        });
    }

    let expanded = quote! {
        match app {
            #cases
            _ => get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        }
    };

    proc_macro::TokenStream::from(expanded)
}
