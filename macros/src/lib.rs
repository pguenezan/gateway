use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, spanned::Spanned, Expr, ExprLit, Lit, Member};
use url::Url;

macro_rules! to_compile_error {
    ($span:expr, $msg:expr) => {
        proc_macro::TokenStream::from(syn::Error::new($span, $msg).to_compile_error())
    };
}

fn expr_to_str(expr: &Expr) -> String {
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

fn check_field(name: &str, value: &str) -> Result<String, String> {
    match name {
        "app_name" => {
            if value.len() < 2 {
                return Err(format!("app_name: {} must be at least 2 characters", value));
            }
            if !value.starts_with('/') {
                return Err(format!("app_name: {} should start with `/`", value));
            }
            Ok(value.to_string())
        }
        "host" => match Url::parse(&format!("http://{}", value)) {
            Err(_) => Err(format!("host: {} isn't valid", value)),
            Ok(_) => Ok(value.to_string()),
        },
        name => Err(format!("unknown field name: {}", name)),
    }
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
                    let name = ident.to_string();
                    if content.contains_key(&name) {
                        return to_compile_error!(ident.span(), "is already defined");
                    }
                    match check_field(&name, &expr_to_str(&field.expr)) {
                        Ok(value) => content.insert(name, value),
                        Err(msg) => return to_compile_error!(field.expr.span(), msg),
                    };
                }
                _ => return to_compile_error!(field.span(), "field should be named"),
            }
        }

        let app_name = match content.get("app_name") {
            Some(app_name) => app_name,
            None => return to_compile_error!(structure.path.span(), "missing field `app_name`"),
        };
        let host = match content.get("host") {
            Some(host) => host,
            None => return to_compile_error!(structure.path.span(), "missing field `host`"),
        };

        cases.extend(quote! {
            #app_name => {
                let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_path);
                match uri_string.parse() {
                    Ok(uri) => *req.uri_mut() = uri,
                    Err(_) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
                };
                inject_headers(req.headers_mut(), &claims);
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
