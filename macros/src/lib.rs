use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Expr};

mod api;
mod endpoint;
mod util;

use api::{parse_apis, ApiMode, Api};

fn check_chain_to(chain_to: &str, apis: &HashMap<String, Api>) -> Result<String, String> {
    // TODO check endpoints validity (for chain)
    // len > 2 && start with / has app
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

    let forward_request = quote! {
        match uri_string.parse() {
            Ok(uri) => *req.uri_mut() = uri,
            Err(_) => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
        };
        inject_headers(req.headers_mut(), &claims);
        match client.request(req).await {
            Ok(response) => {
                timer.observe_duration();
                Ok(response)
            },
            Err(_) => get_response!(StatusCode::BAD_GATEWAY, BADGATEWAY),
        }
    };

    let mut cases = TokenStream::new();

    for (app_name, api) in apis {
        let host = api.host;

        cases.extend(match api.mode {
            ApiMode::ForwardStrict => {
                let mut endpoint_cases = TokenStream::new();
                for endpoint in api.endpoints.unwrap() {
                    match endpoint.chain_to {
                        Some(chain_to) => {

                        },
                        None => (),
                    };

                    let path = endpoint.path;
                    let method = endpoint.method;
                    endpoint_cases.extend(quote!{
                        (#path, #method) => {
                            #forward_request
                        },
                    });
                }
                quote! {
                    #app_name => {
                        let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_uri);
                        match (forwarded_path, method_str) {
                            #endpoint_cases
                            _ => get_response!(StatusCode::NOT_FOUND, NOTFOUND),
                        }
                    },
                }
            },
            ApiMode::ForwardAll => {
                quote! {
                    #app_name => {
                        let uri_string = format!(concat!("http://", #host, "/{}"), forwarded_uri);
                        #forward_request
                    },
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
