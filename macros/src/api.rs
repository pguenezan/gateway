use std::collections::HashMap;
use std::str::FromStr;

use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::{spanned::Spanned, Expr, ExprLit, Lit, Member};
use url::Url;

use crate::endpoint::{parse_endpoints, Endpoint};
use crate::util::to_array;

macro_rules! to_compile_error {
    ($span: expr, $msg: expr) => {
        Err(proc_macro::TokenStream::from(
            syn::Error::new($span, $msg).to_compile_error(),
        ))
    };
}

pub enum ApiMode {
    ForwardAll,
    ForwardStrict,
}

impl FromStr for ApiMode {
    type Err = ();
    fn from_str(input: &str) -> Result<ApiMode, ()> {
        match input {
            "forward_all" => Ok(ApiMode::ForwardAll),
            "forward_strict" => Ok(ApiMode::ForwardStrict),
            _ => Err(()),
        }
    }
}

pub struct Api {
    pub app_name: String,
    pub host: String,
    pub mode: ApiMode,
    pub endpoints: Option<Vec<Endpoint>>,
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
        "mode" => match ApiMode::from_str(value) {
            Ok(_) => Ok(value.to_string()),
            Err(_) => Err(format!("unknown mode: {}", value)),
        },
        name => Err(format!("unknown field name: {}", name)),
    }
}

pub fn parse_apis(input: Expr) -> Result<HashMap<String, Api>, proc_macro::TokenStream> {
    let mut apis = HashMap::new();

    let array = to_array(input)?;
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
        let mut endpoints: Option<Vec<Endpoint>> = None;
        for field in structure.fields {
            match field.member {
                Member::Named(ident) => {
                    let name = ident.to_string();
                    if content.contains_key(&name) {
                        return to_compile_error!(ident.span(), "is already defined");
                    }
                    if name == "endpoints" {
                        match parse_endpoints(field.expr) {
                            Ok(parse_endpoints) => endpoints = Some(parse_endpoints),
                            Err(e) => return Err(e),
                        };
                    } else {
                        match check_field(&name, &expr_to_str(&field.expr)) {
                            Ok(value) => content.insert(name, value),
                            Err(msg) => return to_compile_error!(field.expr.span(), msg),
                        };
                    }
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
        let mode = match content.get("mode") {
            Some(mode) => mode,
            None => return to_compile_error!(structure.path.span(), "missing field `mode`"),
        };

        let api = Api {
            app_name: app_name.to_string(),
            host: host.to_string(),
            mode: ApiMode::from_str(mode).unwrap(),
            endpoints,
        };

        match (&api.endpoints, &api.mode) {
            (Some(_), ApiMode::ForwardAll) => {
                return to_compile_error!(
                    structure.path.span(),
                    "mode `forward_all` doesn't allow endpoints list"
                )
            }
            (None, ApiMode::ForwardStrict) => {
                return to_compile_error!(
                    structure.path.span(),
                    "mode `forward_strict` must have endpoints list"
                )
            }
            _ => (),
        };

        apis.insert(api.app_name.clone(), api);
    }

    Ok(apis)
}
