use std::collections::BTreeSet;
use std::env;
use std::iter;
use std::{cmp, path::Path};

use anyhow::bail;
use proc_macro2::TokenStream;
use quote::quote;
use regex::{escape, Regex};
use syn::{parse_macro_input, LitStr};

mod api;
mod endpoint;

use api::{parse_apis, Api, ApiMode};
use endpoint::Endpoint;

fn get_permission_check(
    api: &Api,
    full_path: Option<&str>,
    method_str: Option<&str>,
) -> TokenStream {
    match (full_path, method_str) {
        (None, None) => quote! {
            let perm = format!("{}::{}::{}", &app[1..], method_str, forwarded_path);
            match perm_lock.read().await.get(&perm) {
                Some(users) if users.contains(&claims.token_id) => (),
                _ => {
                    return get_response(StatusCode::FORBIDDEN, &FORBIDDEN, &labels, &start_time, &req_size);
                },
            }
        },
        (Some(full_path), Some(method_str)) => {
            let re = Regex::new("\\{[^/]*\\}").unwrap();
            if let ApiMode::ForwardStrict(endpoints) = &api.mode {
                for endpoint in endpoints {
                    if endpoint.path == full_path {
                        let perm_path = re.replace_all(&endpoint.path, "{}");
                        let app = &api.app_name[1..];
                        let perm = format!("{}::{}::{}", app, method_str, perm_path);

                        return quote! {
                            println!("checking perm {} for {}", #perm, &claims.token_id);
                            match perm_lock.read().await.get(#perm) {
                                Some(users) if users.contains(&claims.token_id) => (),
                                _ => {
                                    return get_response(StatusCode::FORBIDDEN, &FORBIDDEN, &labels, &start_time, &req_size);
                                },
                            }
                            println!("{} ({}) => {}", claims.preferred_username, claims.token_id, #perm);
                        };
                    }
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
        let role_read = role_lock.read().await;
        let roles = match role_read.get(&claims.token_id) {
            None => "",
            Some(roles) => match roles.get(&#app_name[1..]) {
                None => "",
                Some(roles) => &roles,
            },
        };
        inject_headers(req.headers_mut(), &claims, roles, &token_type);
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

fn check_for_conflicts(api: &Api) -> anyhow::Result<()> {
    if let ApiMode::ForwardStrict(endpoints) = &api.mode {
        let paths: BTreeSet<(String, String)> = endpoints
            .iter()
            .map(|e| (e.path.clone(), e.method.clone()))
            .collect();

        if endpoints.len() != paths.len() {
            bail!("duplicate endpoints in {}", api.app_name);
        }

        let to_regex = Regex::new("\\\\\\{[^/]*\\\\\\}").unwrap();
        for endpoint in endpoints {
            let re = Regex::new(&format!(
                "^{}$",
                to_regex.replace_all(&escape(&endpoint.path), "[^/]+")
            ))
            .unwrap();
            for (path_to_check, _) in &paths {
                if *path_to_check != endpoint.path && re.is_match(&path_to_check) {
                    bail!(
                        "endpoint `{}` conflicts with `{}`",
                        path_to_check,
                        endpoint.path
                    );
                }
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

fn get_common_prefix(paths: &[&str]) -> Option<String> {
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

fn get_trunc_path(paths: &BTreeSet<(String, String, String)>) -> Vec<&str> {
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
    paths: &BTreeSet<(String, String, String)>,
    api: &Api,
    depth: usize,
) -> TokenStream {
    let mut output = TokenStream::new();
    let shift: String = iter::repeat(" ").take(depth).collect();
    for (path, _, _) in paths {
        if !has_param_at_start(path) {
            let next_prefix = get_next_prefix(path);
            let mut prefixed_paths = BTreeSet::new();
            let mut reaming_paths = BTreeSet::new();
            for (path_to_select, full_path, method) in paths {
                if path_to_select == next_prefix {
                    let forward_request = get_forward_request(api, Some(full_path), Some(method));
                    output.extend(quote! {
                        if forwarded_path == #path_to_select && method_str == #method {
                            println!("{}match full '{}'", #shift, #path_to_select);
                            #forward_request
                        }
                    });
                } else if let Some(stripped) = path_to_select.strip_prefix(next_prefix) {
                    prefixed_paths.insert((
                        stripped.to_string(),
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
            if !prefixed_paths.is_empty() {
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
            if !reaming_paths.is_empty() {
                output.extend(generate_case_path_tree_test(&reaming_paths, api, depth));
            }
            return output;
        }
    }
    let mut new_paths = BTreeSet::new();
    for (path, full_path, method) in paths {
        let skip_size = path.find('/').unwrap();
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
            },
            None => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
        }
    });
    output
}

fn generate_case_path_tree_test(
    paths: &BTreeSet<(String, String, String)>,
    api: &Api,
    depth: usize,
) -> TokenStream {
    let mut output = TokenStream::new();
    if paths.is_empty() {
        return output;
    }

    let shift: String = iter::repeat(" ").take(depth).collect();
    let trunc_paths = &get_trunc_path(paths);
    match get_common_prefix(&trunc_paths) {
        None => {
            output.extend(handle_no_common_prefix(paths, api, depth));
        }
        Some(common_prefix) => {
            let mut new_paths = BTreeSet::new();
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
            if !new_paths.is_empty() {
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

fn generate_forward_strict(api: &Api, endpoints: &[Endpoint]) -> TokenStream {
    let app_name = &api.app_name;
    let mut paths: BTreeSet<(String, String, String)> = BTreeSet::new();
    for endpoint in endpoints {
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

fn include_file(path: &Path) -> String {
    let root = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let root_path = Path::new(&root);

    let full_path = root_path.join(path);

    match std::fs::read_to_string(&full_path) {
        Ok(s) => s,
        Err(e) => panic!("Failed to read `{}`: {}", full_path.display(), e),
    }
}

#[proc_macro]
pub fn gateway_config(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as LitStr);

    let file_path = input.value();
    let file_path = Path::new(&file_path);

    let file_content = include_file(file_path);

    let apis = match parse_apis(&file_content) {
        Ok(apis) => apis,
        Err(err) => {
            return proc_macro::TokenStream::from(
                syn::Error::new(input.span(), format!("error deserializing config: {}", err))
                    .to_compile_error(),
            )
        }
    };

    let mut cases = TokenStream::new();
    for api in apis {
        match check_for_conflicts(&api) {
            Ok(_) => (),
            Err(msg) => panic!("{}", msg),
        };

        cases.extend(match &api.mode {
            ApiMode::ForwardStrict(endpoints) => generate_forward_strict(&api, &endpoints),
            ApiMode::ForwardAll => generate_forward_all(&api),
        });
    }

    let expanded = quote! {
        match app {
            #cases
            _ => { return get_response(StatusCode::NOT_FOUND, &NOTFOUND, &labels, &start_time, &req_size); },
        }
    };

    expanded.into()
}
