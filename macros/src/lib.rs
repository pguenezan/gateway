use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream;
use quote::quote;
use regex::{escape, Regex};
use syn::{parse_macro_input, Expr};

mod api;
mod endpoint;
mod util;

use api::{parse_apis, Api, ApiMode};

fn get_permission_check(
    api: &Api,
    full_path: Option<&str>,
    method_str: Option<&str>,
) -> TokenStream {
    match (full_path, method_str) {
        (None, None) => quote! {
            let perm = format!("{}::{}::{}", &app[1..], method_str, forwarded_path);

            let labels = [&app[1..], forwarded_path, method_str];
            HTTP_COUNTER.with_label_values(&labels).inc();
            let timer = HTTP_REQ_HISTOGRAM.with_label_values(&labels).start_timer();

            if !claims.roles.contains(&perm) {
                return get_response!(StatusCode::FORBIDDEN, FORBIDDEN);
            }
        },
        (Some(full_path), Some(method_str)) => {
            let re = Regex::new("\\{[^/]*\\}").unwrap();
            for endpoint in api.endpoints.as_ref().unwrap() {
                if endpoint.path == full_path {
                    let perm_path = re.replace_all(&endpoint.path, "{}");
                    let app = &api.app_name[1..];
                    let perm = format!("{}::{}::{}", app, method_str, perm_path);

                    return quote! {
                        let labels = [#app, #perm_path, #method_str];
                        HTTP_COUNTER.with_label_values(&labels).inc();
                        let timer = HTTP_REQ_HISTOGRAM.with_label_values(&labels).start_timer();

                        if !claims.roles.contains(&#perm.to_owned()) {
                            return get_response!(StatusCode::FORBIDDEN, FORBIDDEN);
                        }

                        println!("{} ({}) => {}", claims.preferred_username, claims.sub, #perm);
                    };
                }
            }
            panic!("Could not find endpoint for path `{}`", full_path);
        }
        (_, _) => {
            panic!("wrong number of arguments");
        }
    }
}

fn get_forward_request(
    api: &Api,
    full_path: Option<&str>,
    method_str: Option<&str>,
) -> TokenStream {
    let host = &api.host;

    let check_perm = get_permission_check(api, full_path, method_str);
    let role_prefix = format!("{}::roles::", api.app_name);

    quote! {
        #check_perm
        let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_uri);
        println!("{}: {}", #method_str, uri_string);
        match uri_string.parse() {
            Ok(uri) => *req.uri_mut() = uri,
            Err(_) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        };
        inject_headers(req.headers_mut(), &claims, #role_prefix, token_type);
        match client.request(req).await {
            Ok(mut response) => {
                timer.observe_duration();
                inject_cors(response.headers_mut());
                return Ok(response)
            },
            Err(error) => {
                println!("502 for {}: {:?}", uri_string, error);
                return get_response!(StatusCode::BAD_GATEWAY, BADGATEWAY)
            },
        }
    }
}

fn check_chain_to(chain_to: &str, apis: &HashMap<String, Api>) -> Result<(), String> {
    let app = &chain_to[..1 + chain_to[1..].find('/').unwrap()];
    let path = &chain_to[app.len()..];

    let api = match apis.get(app) {
        Some(api) => api,
        None => return Err(format!("chain_to: `{}` app doesn't exists", chain_to)),
    };

    if api.mode != ApiMode::ForwardStrict {
        return Err(format!(
            "chain_to: `{}` app mode must be `forward_strict`",
            chain_to
        ));
    }

    for endpoint in api.endpoints.as_ref().unwrap() {
        if endpoint.path == path {
            if endpoint.chain_to != None {
                return Err(format!(
                    "chain_to: `{}` cannot be a chainned endpoint",
                    chain_to
                ));
            }
            if endpoint.method != "POST" {
                return Err(format!("chain_to: `{}` must be POST endpoint", chain_to));
            }
            return Ok(());
        }
    }

    Err(format!("chain_to: `{}` unknown endpoint", chain_to))
}

fn check_for_conflicts(api: &Api) -> Result<(), String> {
    if api.mode == ApiMode::ForwardAll {
        return Ok(());
    }

    let paths: HashSet<(String, String)> = api
        .endpoints
        .as_ref()
        .unwrap()
        .iter()
        .map(|e| (e.path.clone(), e.method.clone()))
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
        for (path_to_check, _) in &paths {
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

fn filter_common_paths(
    paths: &HashSet<(String, String, String)>,
) -> Option<(String, HashSet<(String, String, String)>)> {
    let has_param = Regex::new("\\{[^/]*\\}").unwrap();
    for (path, _, _) in paths {
        if has_param.is_match(path) {
            let mut common_paths = HashSet::new();
            let prefix = &path[..has_param.find(path).unwrap().start()];
            for (other_path, full_path, method) in paths {
                if other_path.starts_with(prefix) {
                    common_paths.insert((
                        other_path.to_string(),
                        full_path.to_string(),
                        method.to_string(),
                    ));
                }
            }
            return Some((prefix.to_string(), common_paths));
        }
    }
    None
}

fn filter_prefix(
    prefix: &str,
    paths: &HashSet<(String, String, String)>,
) -> HashSet<(String, String, String)> {
    let mut filtered = HashSet::new();
    for (path, full_path, method) in paths {
        let suffix = &path[prefix.len()..];
        filtered.insert((
            suffix.to_string(),
            full_path.to_string(),
            method.to_string(),
        ));
    }
    filtered
}

fn handle_prefixed(
    paths: &HashSet<(String, String, String)>,
    prefix_len: usize,
    api: &Api,
) -> TokenStream {
    let has_capture_first = Regex::new("^\\{[^/]*\\}").unwrap();
    let mut simple_cases = TokenStream::new();
    for (path, full_path, method) in paths {
        if !has_capture_first.is_match(&path) {
            let forward_request = get_forward_request(api, Some(full_path), Some(method));
            simple_cases.extend(quote! {
                (#path, #method) => {
                    #forward_request
                },
            });
        }
    }

    let mut reaming_path = HashSet::new();
    for (path, full_path, method) in paths {
        if has_capture_first.is_match(&path) {
            reaming_path.insert((
                path[has_capture_first.find(path).unwrap().end()..].to_string(),
                full_path.to_string(),
                method.to_string(),
            ));
        }
    }

    let (cases, partial) = generate_case_path_tree(&reaming_path, api);

    quote! {
        match (&forwarded_path[#prefix_len..], method_str) {
            #simple_cases
            _ => (),
        };
        match &forwarded_path[#prefix_len..].find('/') {
            Some(0) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
            Some(slash_index) => {
                forwarded_path = &forwarded_path[#prefix_len + slash_index..];
                match (forwarded_path, method_str) {
                    #cases
                    _ => (),
                };
                #partial
                return get_response!(StatusCode::NOT_FOUND, NOTFOUND)
            },
            None => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        }
    }
}

fn generate_case_path_tree(
    paths: &HashSet<(String, String, String)>,
    api: &Api,
) -> (TokenStream, TokenStream) {
    match filter_common_paths(paths) {
        None => {
            let mut tokens = TokenStream::new();
            for (path, full_path, method) in paths {
                let forward_request = get_forward_request(api, Some(full_path), Some(method));
                tokens.extend(quote! {
                    (#path, #method) => {
                        #forward_request
                    },
                });
            }
            (tokens, TokenStream::new())
        }
        Some((prefix, common_paths)) => {
            let prefixed_paths =
                handle_prefixed(&filter_prefix(&prefix, &common_paths), prefix.len(), api);
            let (recursion_cases, recursion_partial) = generate_case_path_tree(
                &paths
                    .difference(&common_paths)
                    .map(|(p, f, m)| (p.to_string(), f.to_string(), m.to_string()))
                    .collect(),
                api,
            );
            let mut partial = quote! {
                if forwarded_path.starts_with(#prefix) {
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
    let forward_request = get_forward_request(&api, None, None);
    quote! {
        #app_name => {
            #forward_request
        },
    }
}

fn generate_forward_strict(api: &Api) -> TokenStream {
    let app_name = &api.app_name;
    let mut paths: HashSet<(String, String, String)> = HashSet::new();
    for endpoint in api.endpoints.as_ref().unwrap() {
        paths.insert((
            endpoint.path.clone(),
            endpoint.path.clone(),
            endpoint.method.clone(),
        ));
    }
    let (cases, partial) = generate_case_path_tree(&paths, &api);
    quote! {
        #app_name => {
            match (forwarded_path, method_str) {
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
    for api in apis.values() {
        match check_for_conflicts(&api) {
            Ok(_) => (),
            Err(msg) => panic!("{}", msg),
        };

        if api.mode == ApiMode::ForwardStrict {
            for endpoint in api.endpoints.as_ref().unwrap() {
                match &endpoint.chain_to {
                    Some(chain_to) => {
                        for path in chain_to {
                            match check_chain_to(path, &apis) {
                                Ok(_) => (),
                                Err(msg) => panic!("{}", msg),
                            }
                        }
                    }
                    None => (),
                }
            }
        }

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
