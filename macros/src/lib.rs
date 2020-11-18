use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use regex::{escape, Regex};
use syn::{parse_macro_input, Expr};

mod api;
mod endpoint;
mod util;

use api::{parse_apis, Api, ApiMode};

fn get_forward_request(host: &str) -> TokenStream {
    quote! {
        let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_uri);
        match uri_string.parse() {
            Ok(uri) => *req.uri_mut() = uri,
            Err(_) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        };
        inject_headers(req.headers_mut(), &claims);
        match client.request(req).await {
            Ok(response) => {
                timer.observe_duration();
                return Ok(response)
            },
            Err(_) => return get_response!(StatusCode::BAD_GATEWAY, BADGATEWAY),
        }
    }
}

// fn check_chain_to(chain_to: &str, apis: &HashMap<String, Api>) -> Result<String, String> {
//     // TODO check endpoints validity (for chain)
//     // len > 2 && start with / has app
// }

fn check_for_conflicts(api: &Api) -> Result<(), String> {
    if api.mode == ApiMode::ForwardAll {
        return Ok(());
    }

    let paths: HashSet<String> = api
        .endpoints
        .as_ref()
        .unwrap()
        .iter()
        .map(|e| e.path.clone())
        .collect();
    if api.endpoints.as_ref().unwrap().len() != paths.len() {
        return Err(format!("duplicate endpoints in {}", api.app_name));
    }

    let to_regex = Regex::new("\\\\\\{[^/]*\\\\\\}").unwrap();
    for endpoint in api.endpoints.as_ref().unwrap() {
        let re = Regex::new(&format!(
            "^{}$",
            to_regex.replace_all(&escape(&endpoint.path), "[^/]+")
        ))
        .unwrap();
        for path_to_check in &paths {
            if *path_to_check != endpoint.path && re.is_match(&path_to_check) {
                return Err(format!(
                    "endpoint `{}` conflicts with `{}`",
                    path_to_check, endpoint.path
                ));
            }
        }
    }
    Ok(())
}

fn filter_common_paths(paths: &HashSet<String>) -> Option<(String, HashSet<String>)> {
    let has_param = Regex::new("\\{[^/]*\\}").unwrap();
    for path in paths {
        if has_param.is_match(path) {
            let mut common_paths = HashSet::new();
            let prefix = &path[..has_param.find(path).unwrap().start()];
            for other_path in paths {
                if other_path.starts_with(prefix) {
                    common_paths.insert(other_path.to_string());
                }
            }
            return Some((prefix.to_string(), common_paths));
        }
    }
    return None;
}

fn filter_prefix(prefix: &String, paths: &HashSet<String>) -> HashSet<String> {
    let mut filtered = HashSet::new();
    for path in paths {
        let suffix = &path[prefix.len()..];
        filtered.insert(suffix.to_string());
    }
    return filtered;
}

fn handle_prefixed(paths: &HashSet<String>, prefix_len: usize) -> TokenStream {
    let has_capture_first = Regex::new("^\\{[^/]*\\}").unwrap();
    let mut simple_cases = TokenStream::new();
    let forward_request = get_forward_request("TODO_HOST:80");
    for path in paths {
        if !has_capture_first.is_match(&path) {
            simple_cases.extend(quote! {
                #path => {
                    #forward_request
                },
            });
        }
    }

    let mut reaming_path = HashSet::new();
    for path in paths {
        if has_capture_first.is_match(&path) {
            reaming_path.insert(path[has_capture_first.find(path).unwrap().end()..].to_string());
        }
    }

    let (cases, partial) = generate_case_path_tree(&reaming_path);

    quote! {
        println!("inside {}", #prefix_len);
        match &forwarded_path[#prefix_len..] {
            #simple_cases
            _ => (),
        };
        match &forwarded_path[#prefix_len..].find('/') {
            Some(0) => {
                println!("zero ?");
                return get_response!(StatusCode::NOT_FOUND, NOTFOUND)
            }
            Some(slash_index) => {
                forwarded_path = &forwarded_path[#prefix_len + slash_index..];
                println!("fpath = {}, slash_index = {}", forwarded_path, slash_index);
                match forwarded_path {
                    #cases
                    _ => (),
                };
                #partial
                return get_response!(StatusCode::NOT_FOUND, NOTFOUND)
            },
            None => {
                println!("no slash ?");
                return get_response!(StatusCode::NOT_FOUND, NOTFOUND)
            },
        }
    }
}

fn generate_case_path_tree(paths: &HashSet<String>) -> (TokenStream, TokenStream) {
    let forward_request = get_forward_request("TODO_HOST:80");
    match filter_common_paths(paths) {
        None => {
            let mut tokens = TokenStream::new();
            for path in paths {
                tokens.extend(quote! {
                    #path => {
                        #forward_request
                    },
                });
            }
            (tokens, TokenStream::new())
        }
        Some((prefix, common_paths)) => {
            let prefixed_paths =
                handle_prefixed(&filter_prefix(&prefix, &common_paths), prefix.len());
            let (recursion_cases, recursion_partial) = generate_case_path_tree(
                &paths
                    .difference(&common_paths)
                    .map(|s| s.to_string())
                    .collect(),
            );
            let mut partial = quote! {
                if forwarded_path.starts_with(#prefix) {
                    println!("inside: {}", #prefix);
                    #prefixed_paths
                }
            };
            partial.extend(recursion_partial);
            (recursion_cases, partial)
        }
    }
}

fn generate_forward_all(api: &Api) -> TokenStream {
    let app_name = &api.app_name;
    let forward_request = get_forward_request(&api.host);
    quote! {
        #app_name => {
            #forward_request
        },
    }
}

fn generate_forward_strict(api: &Api) -> TokenStream {
    let app_name = &api.app_name;
    let paths: &HashSet<String> = &api
        .endpoints
        .as_ref()
        .unwrap()
        .iter()
        .map(|e| e.path.clone())
        .collect();
    let (cases, partial) = generate_case_path_tree(paths);
    quote! {
        #app_name => {
            println!("inside app: {}", #app_name);
            match forwarded_path {
                #cases
                _ => (),
            };
            #partial
            return get_response!(StatusCode::NOT_FOUND, NOTFOUND)
        },
    }
}

#[proc_macro]
pub fn gateway_config(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as Expr);

    let apis = match parse_apis(input) {
        Ok(apis) => apis,
        Err(e) => {
            return e;
        }
    };

    let mut cases = TokenStream::new();
    for (_, api) in apis {
        match check_for_conflicts(&api) {
            Ok(_) => (),
            Err(msg) => panic!("{}", msg),
        };

        cases.extend(match api.mode {
            ApiMode::ForwardStrict => generate_forward_strict(&api),
            ApiMode::ForwardAll => generate_forward_all(&api),
        });
    }

    let expanded = quote! {
        match app {
            #cases
            _ => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        }
    };

    proc_macro::TokenStream::from(expanded)
}
