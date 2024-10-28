use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use http_body::SizeHint;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Incoming};
use hyper::header::{
    HeaderValue, ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
    ACCESS_CONTROL_MAX_AGE, AUTHORIZATION, CONTENT_TYPE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use hyper_tungstenite::is_upgrade_request;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use prometheus::{Encoder, TextEncoder};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use url::Url;

mod api;
mod auth;
mod endpoint;
mod fetch_crd;
mod metrics;
mod permission;
mod route;
mod runtime_config;
mod websocket;

use crate::api::{ApiDefinition, ApiMode};
use crate::auth::{get_claims, Claims};
use crate::endpoint::Endpoint;
use crate::fetch_crd::update_api;
use crate::metrics::commit_http_metrics;
use crate::permission::{get_perm, has_perm, update_perm};
use crate::route::Node;
use crate::runtime_config::RUNTIME_CONFIG;
use crate::websocket::handle_upgrade;

#[macro_use]
extern crate log;

type BoxResponse<D> = Response<BoxBody<D, anyhow::Error>>;

static OK: &[u8] = b"Ok";
static NOT_FOUND: &[u8] = b"Not Found";
static FORBIDDEN: &[u8] = b"Forbidden";
static BAD_GATEWAY: &[u8] = b"Bad Gateway";
static NO_CONTENT: &[u8] = b"";

fn into_boxed_response<B>(response: Response<B>) -> BoxResponse<B::Data>
where
    B: Body + Send + Sync + 'static,
    B::Error: std::error::Error + Send + Sync,
{
    response.map(|body| body.map_err(|err| anyhow!("Invalid Body: {err}")).boxed())
}

#[inline(always)]
fn get_response(
    app: &str,
    method: &Method,
    status_code: StatusCode,
    content: &'static [u8],
    start_time: &Instant,
    req_size: &SizeHint,
) -> Result<Response<Full<Bytes>>> {
    let response: Response<Full<Bytes>> = Response::builder()
        .status(status_code)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, "*")
        .header(ACCESS_CONTROL_EXPOSE_HEADERS, "Location, Retry-After")
        .header(ACCESS_CONTROL_ALLOW_CREDENTIALS, "true")
        .header(ACCESS_CONTROL_MAX_AGE, 86400)
        .body(content.into())?;

    commit_http_metrics(
        app,
        method,
        start_time,
        status_code,
        req_size,
        &response.body().size_hint(),
    );

    debug!("event='Response built'");
    Ok(response)
}

fn inject_cors(headers: &mut HeaderMap<HeaderValue>) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
}

fn inject_headers(
    headers: &mut HeaderMap<HeaderValue>,
    claims: &Claims,
    app_user_roles: &str,
    token_type: &str,
) {
    headers.remove("Authorization");
    if let Ok(value) = claims.token_id.parse() {
        headers.insert("X-Forwarded-User", value);
    } else {
        info!("event='No token_id in token'");
    }
    if let Ok(value) = claims.preferred_username.parse() {
        headers.insert("X-Forwarded-User-Username", value);
    } else {
        info!("event='No username in token'");
    }
    if let Ok(value) = claims.given_name.parse() {
        headers.insert("X-Forwarded-User-First-Name", value);
    } else {
        info!("event='No user first name in token'");
    }
    if let Ok(value) = claims.family_name.parse() {
        headers.insert("X-Forwarded-User-Last-Name", value);
    } else {
        info!("event='No user last name in token'");
    }
    if let Ok(value) = claims.email.parse() {
        headers.insert("X-Forwarded-User-Email", value);
    } else {
        info!("event='No user email in token'");
    }
    if let Ok(value) = app_user_roles.parse() {
        headers.insert("X-Forwarded-User-Roles", value);
    } else {
        info!("event='No user roles in token'");
    }
    if let Ok(value) = token_type.parse() {
        headers.insert("X-Forwarded-User-Type", value);
    } else {
        info!("event='No token type in token'");
    }
}

async fn metrics() -> Result<Response<Full<Bytes>>> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(buffer.into())
        .unwrap();

    Ok(response)
}

async fn health() -> Result<Response<Full<Bytes>>> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, "*")
        .body(OK.into())
        .unwrap())
}

#[allow(clippy::too_many_arguments)]
async fn call(
    mut req: Request<Incoming>,
    client: &Client<HttpConnector, Incoming>,
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    endpoint: &Endpoint,
    api: &ApiDefinition,
    claims: &Claims,
    app: &str,
    start_time: &Instant,
    req_size: &SizeHint,
    http_uri_string: &str,
    ws_uri_string: &str,
    token_type: &str,
) -> Result<BoxResponse<Bytes>> {
    let path = &req.uri().path().to_owned();

    if endpoint.check_permission
        && !has_perm(perm_lock, &endpoint.permission, &claims.token_id).await
    {
        info!(
            "method='{}' path='{}' uri='{}' status_code='403' user_sub='{}' token_id='{}' error='Does not have the permission' perm='{}'",
            req.method(),
            path,
            http_uri_string,
            claims.sub,
            claims.token_id,
            &endpoint.permission,
        );

        return get_response(
            app,
            req.method(),
            StatusCode::FORBIDDEN,
            FORBIDDEN,
            start_time,
            req_size,
        )
        .map(into_boxed_response);
    }

    if endpoint.is_websocket && is_upgrade_request(&req) {
        return handle_upgrade(app, req, start_time, req_size, ws_uri_string)
            .await
            .map(into_boxed_response);
    }

    if endpoint.is_websocket {
        debug!("event='Websocket require upgrade'");

        return get_response(
            app,
            req.method(),
            StatusCode::UPGRADE_REQUIRED,
            NO_CONTENT,
            start_time,
            req_size,
        )
        .map(into_boxed_response);
    }

    match http_uri_string.parse() {
        Ok(uri) => *req.uri_mut() = uri,
        Err(e) => {
            error!("error='Uri parsing error: {:?}'", e);

            return get_response(
                app,
                req.method(),
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                start_time,
                req_size,
            )
            .map(into_boxed_response);
        }
    };

    let role_read = role_lock.read().await;
    let roles = match role_read.get(&claims.token_id) {
        None => "",
        Some(roles) => match roles.get(&api.spec.app_name[1..]) {
            None => "",
            Some(roles) => roles,
        },
    };

    inject_headers(req.headers_mut(), claims, roles, token_type);
    let method = req.method().clone();

    let request_start_time = Instant::now();

    let response = client.request(req).await;

    let request_duration_ms = request_start_time.elapsed().as_millis();

    match response {
        Ok(mut response) => {
            inject_cors(response.headers_mut());

            commit_http_metrics(
                app,
                &method,
                start_time,
                response.status(),
                req_size,
                &response.size_hint(),
            );

            info!(
                "method='{}' path='{}' uri='{}' status_code='{}' user_sub='{}' token_id='{}' perm='{}' duration='{}ms'",
                method,
                path,
                http_uri_string,
                response.status(),
                claims.sub,
                claims.token_id,
                &endpoint.permission,
                request_duration_ms,
            );

            Ok(into_boxed_response(response))
        }
        Err(error) => {
            warn!(
                "method='{}' path='{}' uri='{}' status_code='502' user_sub='{}' token_id='{}' error='{:?}' perm='{}' duration='{}ms'",
                method,
                path,
                http_uri_string,
                claims.sub,
                claims.token_id,
                error,
                &endpoint.permission,
                request_duration_ms,
            );

            get_response(
                app,
                &method,
                StatusCode::BAD_GATEWAY,
                BAD_GATEWAY,
                start_time,
                req_size,
            )
            .map(into_boxed_response)
        }
    }
}

fn get_auth_from_url(uri: &Uri) -> Option<String> {
    let url = Url::parse(&format!("http://localhost{}", uri.path_and_query()?)).ok()?;
    for (key, value) in url.query_pairs() {
        if key != "_auth_token" {
            continue;
        }
        return Some(format!("Bearer {}", value));
    }
    warn!("event='No authorization header found'");
    None
}

async fn response(
    req: Request<Incoming>,
    client: Client<HttpConnector, Incoming>,
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
) -> Result<BoxResponse<Bytes>> {
    match req.uri().path() {
        "/metrics" => {
            debug!("event='Metrics endpoint'");
            return metrics().await.map(into_boxed_response);
        }
        "/health" => {
            debug!("event='Health endpoint'");
            return health().await.map(into_boxed_response);
        }
        _ => (),
    };

    let start_time = Instant::now();

    let uri = &req.uri().to_owned();
    let path = &req.uri().path().to_owned();
    let req_size = req.size_hint();

    // to handle CORS pre flights
    if req.method() == Method::OPTIONS {
        info!("method='{}' path='{}' uri='{}' status_code='204' user_sub='Not yet decoded' token_id='Not yet decoded'", req.method(), path, uri);
        return get_response(
            "",
            req.method(),
            StatusCode::NO_CONTENT,
            NO_CONTENT,
            &start_time,
            &req_size,
        )
        .map(into_boxed_response);
    }

    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='Not yet decoded' token_id='Not yet decoded' error='No / found'", req.method(), path, uri);
            return get_response(
                "",
                req.method(),
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &start_time,
                &req_size,
            )
            .map(into_boxed_response);
        }
    };
    let app = &path[..slash_index];

    let authorization = match req.headers().get(AUTHORIZATION) {
        None => match get_auth_from_url(req.uri()) {
            None => {
                warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='No authorization header'", req.method(), path, uri);
                return get_response(
                    app,
                    req.method(),
                    StatusCode::FORBIDDEN,
                    FORBIDDEN,
                    &start_time,
                    &req_size,
                )
                .map(into_boxed_response);
            }
            Some(authorization) => authorization,
        },
        Some(authorization) => match authorization.to_str() {
            Err(e) => {
                warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='{}'", req.method(), path, uri, format!("Error in authorization: {:#?}", e));
                return get_response(
                    app,
                    req.method(),
                    StatusCode::FORBIDDEN,
                    FORBIDDEN,
                    &start_time,
                    &req_size,
                )
                .map(into_boxed_response);
            }
            Ok(authorization) => authorization.to_string(),
        },
    };
    let (claims, token_type) = match get_claims(&authorization).await {
        Some(claims) => claims,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='Invalid or no claim'", req.method(), path, uri);
            return get_response(
                app,
                req.method(),
                StatusCode::FORBIDDEN,
                FORBIDDEN,
                &start_time,
                &req_size,
            )
            .map(into_boxed_response);
        }
    };

    let forwarded_uri = match req.uri().path_and_query().map(|x| &x.as_str()[app.len()..]) {
        Some(forwarded_uri) => forwarded_uri,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='Not yet decoded' token_id='Not yet decoded' error='Forward api not found'", req.method(), path, uri);
            return get_response(
                app,
                req.method(),
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &start_time,
                &req_size,
            )
            .map(into_boxed_response);
        }
    };

    let forwarded_path = &req.uri().path()[app.len()..];

    match api_lock.read().await.get(app) {
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='{}' token_id='{}' error='Forward api not found'", req.method(), path, uri, claims.sub, claims.token_id);
            get_response(
                app,
                req.method(),
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &start_time,
                &req_size,
            )
            .map(into_boxed_response)
        }
        Some((api, node)) => match api.spec.mode {
            ApiMode::ForwardAll => {
                let endpoint = Endpoint::from_forward_all(
                    forwarded_path.to_string(),
                    req.method().to_string(),
                    app,
                );
                let http_uri_string = format!("{}{}", &api.spec.uri_http, forwarded_uri);
                let ws_uri_string = format!("{}{}", &api.spec.uri_ws, forwarded_uri);
                call(
                    req,
                    &client,
                    perm_lock,
                    role_lock,
                    &endpoint,
                    api,
                    &claims,
                    app,
                    &start_time,
                    &req_size,
                    &http_uri_string,
                    &ws_uri_string,
                    &token_type,
                )
                .await
            }
            ApiMode::ForwardStrict(_) => {
                match node.match_path(forwarded_path, req.method().as_str()) {
                    None => {
                        warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='{}' token_id='{}' error='Endpoint not found in service'", req.method(), path, uri, claims.sub, claims.token_id);
                        get_response(
                            app,
                            req.method(),
                            StatusCode::NOT_FOUND,
                            NOT_FOUND,
                            &start_time,
                            &req_size,
                        )
                        .map(into_boxed_response)
                    }
                    Some(endpoint) => {
                        let http_uri_string = format!("{}{}", &api.spec.uri_http, forwarded_uri);
                        let ws_uri_string = format!("{}{}", &api.spec.uri_ws, forwarded_uri);
                        call(
                            req,
                            &client,
                            perm_lock,
                            role_lock,
                            endpoint,
                            api,
                            &claims,
                            app,
                            &start_time,
                            &req_size,
                            &http_uri_string,
                            &ws_uri_string,
                            &token_type,
                        )
                        .await
                    }
                }
            }
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let addr: SocketAddr = match RUNTIME_CONFIG.bind_to.parse() {
        Ok(addr) => addr,
        Err(_) => {
            error!("event='Address bind_to is not valid'");
            exit(1);
        }
    };

    // permissions fetching
    let (perm, role) = get_perm().await.unwrap();
    let perm_lock = Arc::new(RwLock::new(perm));
    let role_lock = Arc::new(RwLock::new(role));
    let update_perm = update_perm(perm_lock.clone(), role_lock.clone());

    // apidefinitions fetching
    let api_lock = Arc::new(RwLock::new(HashMap::new()));
    let update_api = update_api(
        api_lock.clone(),
        RUNTIME_CONFIG.crd_label.to_owned(),
        RUNTIME_CONFIG.crds_namespaces.to_owned(),
    );

    // Share a `Client` with all `Service`s
    let client = Client::builder(TokioExecutor::new()).build_http();

    let service = service_fn(move |req| {
        response(
            req,
            client.to_owned(),
            perm_lock.clone(),
            role_lock.clone(),
            api_lock.clone(),
        )
    });

    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|err| anyhow!("Could not listen on {addr}: {err}"))?;

    info!("event='Listening on http://{}'", addr);

    let res = tokio::try_join!(update_perm, update_api, async {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _socket)) => stream,
                Err(err) => {
                    error!("Failed to accept connection: {err:?}");
                    continue;
                }
            };

            let io = TokioIo::new(stream);
            let service = service.clone();

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(io, service)
                    .with_upgrades()
                    .await
                {
                    error!("Failed to serve connection: {err:?}");
                }
            });
        }

        // This part is unreachable but we still define a return value to help
        // type inference of the async block.
        #[allow(unreachable_code)]
        Result::Ok(())
    });

    match res {
        Ok((_, _, _)) => info!("That went well"),
        Err(e) => {
            error!("Error in join: {:?}", e);
            exit(1);
        }
    }

    Ok(())
}
