use std::cmp;
use std::collections::{HashMap, HashSet};
use std::iter;

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
            if !claims.roles.contains(&perm) {
                return get_response(StatusCode::FORBIDDEN, &FORBIDDEN, &labels, &start_time, &req_size);
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
                        if !claims.roles.contains(&#perm.to_owned()) {
                            return get_response(StatusCode::FORBIDDEN, &FORBIDDEN, &labels, &start_time, &req_size);
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
    let host = format!("http://{}{}/", &api.host, &api.forward_path);

    let check_perm = get_permission_check(api, full_path, method_str);
    let role_prefix = format!("{}::roles::", &api.app_name[1..]);
    let app_name = &api.app_name;

    let commit = match (full_path, method_str) {
        (None, None) => quote! {
            commit_metrics(&labels, &start_time, response.status(), &req_size, &response.size_hint());
        },
        (Some(full_path), Some(method_str)) => quote! {
            let local_labels = [#app_name, #full_path, #method_str];
            println!("local_labels = {:?}", local_labels);
            commit_metrics(&local_labels, &start_time, response.status(), &req_size, &response.size_hint());
        },
        (_, _) => {
            panic!("wrong number of arguments");
        }
    };

    let bad_gateway = match (full_path, method_str) {
        (None, None) => quote! {
            return get_response(StatusCode::BAD_GATEWAY, &BADGATEWAY, &labels, &start_time, &req_size);
        },
        (Some(full_path), Some(method_str)) => quote! {
            let local_labels = [#app_name, #full_path, #method_str];
            return get_response(StatusCode::BAD_GATEWAY, &BADGATEWAY, &local_labels, &start_time, &req_size);
        },
        (_, _) => {
            panic!("wrong number of arguments");
        }
    };

    quote! {
        #check_perm
        let uri_string = format!(concat!(#host, "{}"), forwarded_uri);
        println!("{}: {}", method_str, uri_string);
        match uri_string.parse() {
            Ok(uri) => *req.uri_mut() = uri,
            Err(_) => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
        };
        inject_headers(req.headers_mut(), &claims, #role_prefix, token_type);
        match client.request(req).await {
            Ok(mut response) => {
                inject_cors(response.headers_mut());
                #commit
                return Ok(response)
            },
            Err(error) => {
                println!("502 for {}: {:?}", uri_string, error);
                #bad_gateway
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

fn generate_forward_all(api: &Api) -> TokenStream {
    let app_name = &api.app_name;
    let forward_request = get_forward_request(&api, None, None);
    quote! {
        #app_name => {
            #forward_request
        },
    }
}

fn get_prefix_size(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn get_common_prefix(paths: &Vec<&str>) -> Option<String> {
    match paths.len() {
        0 => None,
        1 => Some(paths[0].to_string()),
        _ => {
            let first = &paths[0];
            let mut prefix_size = first.len();
            for path in &paths[1..] {
                prefix_size = cmp::min(prefix_size, get_prefix_size(first, path));
            }
            if prefix_size == 0 {
                return None;
            }
            Some(first[..prefix_size].to_string())
        }
    }
}

fn get_trunc_path(paths: &HashSet<(String, String, String)>) -> Vec<&str> {
    let has_param = Regex::new("\\{[^/]*\\}").unwrap();
    let mut trunc_paths = Vec::new();
    for (path, _, _) in paths {
        if has_param.is_match(path) {
            let prefix = &path[..has_param.find(path).unwrap().start()];
            trunc_paths.push(prefix);
        } else {
            trunc_paths.push(&path);
        }
    }
    trunc_paths
}

fn has_param_at_start(path: &str) -> bool {
    let has_param_at_start = Regex::new("^\\{[^/]*\\}").unwrap();
    has_param_at_start.is_match(path)
}

fn get_next_prefix(path: &str) -> &str {
    let has_param = Regex::new("\\{[^/]*\\}").unwrap();
    match has_param.is_match(path) {
        true => &path[..has_param.find(path).unwrap().start()],
        false => path,
    }
}

fn handle_no_common_prefix(
    paths: &HashSet<(String, String, String)>,
    api: &Api,
    depth: usize,
) -> TokenStream {
    let mut output = TokenStream::new();
    let shift: String = iter::repeat(" ").take(depth).collect();
    for (path, _, _) in paths {
        if !has_param_at_start(path) {
            let next_prefix = get_next_prefix(path);
            let mut prefixed_paths = HashSet::new();
            let mut reaming_paths = HashSet::new();
            for (path_to_select, full_path, method) in paths {
                if path_to_select == next_prefix {
                    let forward_request = get_forward_request(api, Some(full_path), Some(method));
                    output.extend(quote! {
                        if forwarded_path == #path_to_select && method_str == #method {
                            println!("{}match full '{}'", #shift, #path_to_select);
                            #forward_request
                        }
                    });
                } else if path_to_select.starts_with(next_prefix) {
                    prefixed_paths.insert((
                        path_to_select[next_prefix.len()..].to_string(),
                        full_path.to_string(),
                        method.to_string(),
                    ));
                } else {
                    reaming_paths.insert((
                        path_to_select.to_string(),
                        full_path.to_string(),
                        method.to_string(),
                    ));
                }
            }
            if prefixed_paths.len() != 0 {
                let reaming = handle_no_common_prefix(&prefixed_paths, api, depth + 2);
                output.extend(quote!{
                    if forwarded_path.starts_with(#next_prefix) {
                        println!("{}match '{}'", #shift, #next_prefix);
                        forwarded_path = &forwarded_path[#next_prefix.len()..];
                        #reaming
                        return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size);
                    }
                });
            }
            if reaming_paths.len() != 0 {
                output.extend(generate_case_path_tree_test(&reaming_paths, api, depth));
            }
            return output;
        }
    }
    let mut new_paths = HashSet::new();
    for (path, full_path, method) in paths {
        let skip_size = path.find("/").unwrap();
        if !has_param_at_start(path) {
            panic!(
                "`{} `(part of `{}`) should starts with a capture",
                path, full_path
            );
        }
        new_paths.insert((
            path[skip_size..].to_string(),
            full_path.to_string(),
            method.to_string(),
        ));
    }
    let reaming = generate_case_path_tree_test(&new_paths, api, depth + 2);
    let rest_of_path = &paths.iter().next().unwrap().0;
    output.extend(quote!{
        match forwarded_path.find('/') {
            Some(0) => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
            Some(slash_index) => {
                println!("{}skipping until '/' (for capture of '{}'", #shift, #rest_of_path);
                forwarded_path = &forwarded_path[slash_index..];
                #reaming
                return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size);
            },
            None => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
        }
    });
    output
}

fn generate_case_path_tree_test(
    paths: &HashSet<(String, String, String)>,
    api: &Api,
    depth: usize,
) -> TokenStream {
    let mut output = TokenStream::new();
    if paths.len() == 0 {
        return output;
    }

    let shift: String = iter::repeat(" ").take(depth).collect();
    let trunc_paths = &get_trunc_path(paths);
    match get_common_prefix(&trunc_paths) {
        None => {
            output.extend(handle_no_common_prefix(paths, api, depth));
        }
        Some(common_prefix) => {
            let mut new_paths = HashSet::new();
            for (path, full_path, method) in paths {
                if &common_prefix == path {
                    let forward_request = get_forward_request(api, Some(full_path), Some(method));
                    output.extend(quote! {
                        if forwarded_path == #path && method_str == #method {
                            println!("{}match full '{}'", #shift, #path);
                            #forward_request
                        }
                    });
                    continue;
                }
                new_paths.insert((
                    path[common_prefix.len()..].to_string(),
                    full_path.to_string(),
                    method.to_string(),
                ));
            }
            if new_paths.len() != 0 {
                let reaming = handle_no_common_prefix(&new_paths, api, depth + 2);
                output.extend(quote!{
                    if forwarded_path.starts_with(#common_prefix) {
                        println!("{}match '{}'", #shift, #common_prefix);
                        forwarded_path = &forwarded_path[#common_prefix.len()..];
                        #reaming
                        return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size);
                    }
                });
            }
        }
    }
    output
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
    let cases = generate_case_path_tree_test(&paths, &api, 2);
    quote! {
        #app_name => {
            println!("match {} => ({}, {})", #app_name, forwarded_path, method_str);
            #cases
            return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size);
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
            _ => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
        }
    };

    proc_macro::TokenStream::from(expanded)
}
