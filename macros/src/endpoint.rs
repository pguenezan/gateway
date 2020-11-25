use std::collections::HashMap;
use std::str::FromStr;

use hyper::Method;
use proc_macro2::TokenStream;
use quote::ToTokens;
use regex::Regex;
use syn::{spanned::Spanned, Expr, ExprLit, Lit, Member};

use crate::util::to_array;

macro_rules! to_compile_error {
    ($span: expr, $msg: expr) => {
        Err(proc_macro::TokenStream::from(
            syn::Error::new($span, $msg).to_compile_error(),
        ))
    };
}

#[derive(Debug)]
pub struct Endpoint {
    pub path: String,
    pub method: String,
    pub chain_to: Option<Vec<String>>,
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

fn check_for_params(path: &str) -> Result<(), String> {
    let match_param = Regex::new("\\{[^/]+\\}").unwrap();
    let mut mut_path: String = path.to_string();
    while match_param.is_match(&mut_path) {
        let mut param = match_param.find(&mut_path).unwrap().as_str();
        param = &param[1..param.len() - 2];
        if param.contains('{') || param.contains('}') {
            return Err(format!(
                "param: `{}` contains `{{` or `}}` in path `{}`",
                param, path
            ));
        }
        mut_path = match_param.replace(&mut_path, "").to_string();
    }
    if mut_path.contains('{') || mut_path.contains('}') {
        return Err(format!("path: `{}` contains/is missing `{{` or `}}`", path));
    }
    Ok(())
}

fn check_field(name: &str, value: &str) -> Result<String, String> {
    match name {
        "path" => {
            if value.is_empty() {
                return Err(format!("path: {} must be at least 1 characters", value));
            }
            if !value.starts_with('/') {
                return Err(format!("path: {} should start with `/`", value));
            }
            if !value.ends_with('/') {
                return Err(format!("path: {} should end with `/`", value));
            }
            check_for_params(value)?;
            Ok(value.to_string())
        }
        "method" => match Method::from_str(value) {
            Ok(_) => Ok(value.to_string()),
            Err(_) => Err(format!("unknown method: {}", value)),
        },
        name => Err(format!("unknown field name: {}", name)),
    }
}

fn parse_chain_to(input: Expr) -> Result<Vec<String>, proc_macro::TokenStream> {
    let mut chain_to = Vec::new();

    let array = to_array(input)?;
    for elem in array.elems {
        let path = expr_to_str(&elem);
        if !path.starts_with('/') {
            return to_compile_error!(elem.span(), "should start with `/`");
        }
        if !path.ends_with('/') {
            return to_compile_error!(elem.span(), "should end with `/`");
        }
        if path[1..].find('/') == None {
            return to_compile_error!(elem.span(), "should app and endpoint");
        }
        chain_to.push(path);
    }

    Ok(chain_to)
}

pub fn parse_endpoints(input: Expr) -> Result<Vec<Endpoint>, proc_macro::TokenStream> {
    let mut endpoints = Vec::new();

    let array = to_array(input)?;

    for elem in array.elems {
        let structure = match elem {
            Expr::Struct(structure) => structure,
            _ => return to_compile_error!(elem.span(), "not a structure"),
        };
        if structure.path.segments.len() != 1 {
            return to_compile_error!(structure.path.span(), "a single path segment is expected");
        }
        if structure.path.segments[0].ident != "Endpoint" {
            return to_compile_error!(
                structure.path.span(),
                "structure must be of type `Endpoint`"
            );
        }
        let mut content = HashMap::new();
        let mut chain_to = None;
        for field in structure.fields {
            match field.member {
                Member::Named(ident) => {
                    let name = ident.to_string();
                    if content.contains_key(&name) {
                        return to_compile_error!(ident.span(), "is already defined");
                    }
                    if name == "chain_to" {
                        match parse_chain_to(field.expr) {
                            Ok(parse_chain_to) => chain_to = Some(parse_chain_to),
                            Err(e) => return Err(e),
                        }
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

        let path = match content.get("path") {
            Some(path) => path,
            None => return to_compile_error!(structure.path.span(), "missing field `path`"),
        };
        let method = match content.get("method") {
            Some(method) => method,
            None => return to_compile_error!(structure.path.span(), "missing field `method`"),
        };

        endpoints.push(Endpoint {
            path: path.to_string(),
            method: method.to_string(),
            chain_to,
        });
    }

    Ok(endpoints)
}
