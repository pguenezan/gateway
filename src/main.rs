use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::process::exit;
use std::time::Instant;

use http_body::SizeHint;
use hyper::body::HttpBody;
use hyper::client::HttpConnector;
use hyper::header::{
    HeaderValue, ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_MAX_AGE,
    AUTHORIZATION, CONTENT_TYPE,
};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, HeaderMap, Method, Request, Response, Server, StatusCode, Uri};
use hyper_tungstenite::is_upgrade_request;
use std::sync::Arc;
use tokio::sync::RwLock;

use lazy_static::lazy_static;

use anyhow::anyhow;

use prometheus::{
    exponential_buckets, opts, register_counter_vec, register_histogram_vec, CounterVec, Encoder,
    HistogramVec, TextEncoder,
};
use url::Url;

#[macro_use]
extern crate log;

mod api;
use api::{ApiDefinition, ApiMode};

mod endpoint;
mod websocket;

mod route;
use route::Node;

mod auth;
use auth::{get_claims, init_token_sources, Claims};

mod runtime_config;
use runtime_config::{init_runtime_config, RUNTIME_CONFIG};

mod permission;
use permission::{get_perm, has_perm, update_perm};

mod fetch_crd;
use crate::endpoint::Endpoint;
use crate::websocket::handle_upgrade;
use fetch_crd::update_api;

type GenericError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, GenericError>;

static OK: &[u8] = b"Ok";
static NOT_FOUND: &[u8] = b"Not Found";
static FORBIDDEN: &[u8] = b"Forbidden";
static BAD_GATEWAY: &[u8] = b"Bad Gateway";
static NO_CONTENT: &[u8] = b"";

#[inline(always)]
fn commit_metrics(
    labels: &[&str],
    start_time: &Instant,
    status_code: StatusCode,
    req_size: &SizeHint,
    res_size: &SizeHint,
) {
    let full_labels = vec![labels[0], labels[1], status_code.as_str()];

    HTTP_COUNTER.with_label_values(&full_labels).inc();

    HTTP_REQ_LAT_HISTOGRAM
        .with_label_values(&full_labels)
        .observe(start_time.elapsed().as_secs_f64());

    HTTP_REQ_SIZE_HISTOGRAM_LOW
        .with_label_values(&full_labels)
        .observe(req_size.lower() as f64);
    if let Some(size) = req_size.upper() {
        HTTP_REQ_SIZE_HISTOGRAM_HIGH
            .with_label_values(&full_labels)
            .observe(size as f64)
    }

    HTTP_RES_SIZE_HISTOGRAM_LOW
        .with_label_values(&full_labels)
        .observe(res_size.lower() as f64);
    if let Some(size) = req_size.upper() {
        HTTP_RES_SIZE_HISTOGRAM_HIGH
            .with_label_values(&full_labels)
            .observe(size as f64)
    }
}

#[inline(always)]
fn get_response(
    status_code: StatusCode,
    content: &'static [u8],
    labels: &[&str],
    start_time: &Instant,
    req_size: &SizeHint,
) -> Result<Response<Body>> {
    let response = Response::builder()
        .status(status_code)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, "*")
        .header(ACCESS_CONTROL_ALLOW_CREDENTIALS, "true")
        .header(ACCESS_CONTROL_MAX_AGE, 86400)
        .body(content.into())?;

    commit_metrics(
        labels,
        start_time,
        status_code,
        req_size,
        &response.size_hint(),
    );

    debug!("event='Response built'");

    Ok(response)
}

const LABEL_NAMES: [&str; 3] = ["app", "method", "status_code"];

fn get_metric_name(name: &str) -> String {
    format!(
        "gateway_{}_http_{}",
        RUNTIME_CONFIG.get().unwrap().metrics_prefix,
        name
    )
}

lazy_static! {
    static ref HTTP_COUNTER: CounterVec = register_counter_vec!(
        opts!(
            get_metric_name("requests_total"),
            "Number of HTTP requests made."
        ),
        &LABEL_NAMES
    )
    .unwrap();
    static ref HTTP_REQ_LAT_HISTOGRAM: HistogramVec = register_histogram_vec!(
        get_metric_name("request_duration_seconds"),
        "The HTTP request latencies in seconds.",
        &LABEL_NAMES
    )
    .unwrap();
    static ref HTTP_REQ_SIZE_HISTOGRAM_LOW: HistogramVec = register_histogram_vec!(
        get_metric_name("request_size_low_bytes"),
        "The HTTP request size in bytes (lower bound).",
        &LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap();
    static ref HTTP_REQ_SIZE_HISTOGRAM_HIGH: HistogramVec = register_histogram_vec!(
        get_metric_name("request_size_high_bytes"),
        "The HTTP request size in bytes (upper bound).",
        &LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap();
    static ref HTTP_RES_SIZE_HISTOGRAM_LOW: HistogramVec = register_histogram_vec!(
        get_metric_name("response_size_low_bytes"),
        "The HTTP response size in bytes (lower bound).",
        &LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap();
    static ref HTTP_RES_SIZE_HISTOGRAM_HIGH: HistogramVec = register_histogram_vec!(
        get_metric_name("response_size_high_bytes"),
        "The HTTP response size in bytes (upper bound).",
        &LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap();
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
    if cfg!(feature = "remove_authorization_header") {
        headers.remove("Authorization");
    }
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

async fn metrics() -> Result<Response<Body>> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Body::from(buffer))
        .unwrap();

    Ok(response)
}

async fn health() -> Result<Response<Body>> {
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
    mut req: Request<Body>,
    client: &Client<HttpConnector>,
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    endpoint: &Endpoint,
    api: &ApiDefinition,
    claims: &Claims,
    labels: &[&str],
    start_time: &Instant,
    req_size: &SizeHint,
    http_uri_string: &str,
    ws_uri_string: &str,
    token_type: &str,
    method_str: &str,
) -> Result<Response<Body>> {
    let path = &req.uri().path().to_owned();
    match has_perm(perm_lock, &endpoint.permission, &claims.token_id).await {
        false => {
            info!(
                "method='{}' path='{}' uri='{}' status_code='403' user_sub='{}' token_id='{}' error='Does not have the permission' perm='{}'",
                method_str,
                path,
                http_uri_string,
                claims.sub,
                claims.token_id,
                &endpoint.permission,
            );
            get_response(
                StatusCode::FORBIDDEN,
                FORBIDDEN,
                labels,
                start_time,
                req_size,
            )
        }
        true => {
            if endpoint.is_websocket && is_upgrade_request(&req) {
                return handle_upgrade(req, labels, start_time, req_size, ws_uri_string).await;
            }

            if endpoint.is_websocket {
                debug!("event='Websocket require upgrade'");

                return get_response(
                    StatusCode::UPGRADE_REQUIRED,
                    NO_CONTENT,
                    labels,
                    start_time,
                    req_size,
                );
            }

            match http_uri_string.parse() {
                Ok(uri) => *req.uri_mut() = uri,
                Err(e) => {
                    error!("error='Uri parsing error: {:?}'", e);
                    return get_response(
                        StatusCode::NOT_FOUND,
                        NOT_FOUND,
                        labels,
                        start_time,
                        req_size,
                    );
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
            match client.request(req).await {
                Ok(mut response) => {
                    inject_cors(response.headers_mut());
                    commit_metrics(
                        labels,
                        start_time,
                        response.status(),
                        req_size,
                        &response.size_hint(),
                    );
                    info!(
                        "method='{}' path='{}' uri='{}' status_code='{}' user_sub='{}' token_id='{}' perm='{}'",
                        method_str,
                        path,
                        http_uri_string,
                        response.status(),
                        claims.sub,
                        claims.token_id,
                        &endpoint.permission,
                    );
                    Ok(response)
                }
                Err(error) => {
                    warn!(
                        "method='{}' path='{}' uri='{}' status_code='502' user_sub='{}' token_id='{}' error='{:?}' perm='{}'",
                        method_str,
                        path,
                        http_uri_string,
                        claims.sub,
                        claims.token_id,
                        error,
                        &endpoint.permission
                    );
                    get_response(
                        StatusCode::BAD_GATEWAY,
                        BAD_GATEWAY,
                        labels,
                        start_time,
                        req_size,
                    )
                }
            }
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
    req: Request<Body>,
    client: Client<HttpConnector>,
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
) -> Result<Response<Body>> {
    match req.uri().path() {
        "/metrics" => {
            debug!("event='Metrics endpoint'");
            return metrics().await;
        }
        "/health" => {
            debug!("event='Health endpoint'");
            return health().await;
        }
        _ => (),
    };

    let start_time = Instant::now();

    let uri = &req.uri().to_owned();
    let path = &req.uri().path().to_owned();
    let method_str: &str = &req.method().to_string();
    let req_size = req.size_hint();

    // to handle CORS pre flights
    if req.method() == Method::OPTIONS {
        info!("method='{}' path='{}' uri='{}' status_code='204' user_sub='Not yet decoded' token_id='Not yet decoded'", method_str, path, uri);
        return get_response(
            StatusCode::NO_CONTENT,
            NO_CONTENT,
            &["", method_str],
            &start_time,
            &req_size,
        );
    }

    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='Not yet decoded' token_id='Not yet decoded' error='No / found'", method_str, path, uri);
            return get_response(
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &["", method_str],
                &start_time,
                &req_size,
            );
        }
    };
    let app = &path[..slash_index];
    let labels = [app, method_str];

    let authorization = match req.headers().get(AUTHORIZATION) {
        None => match get_auth_from_url(req.uri()) {
            None => {
                warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='No authorization header'", method_str, path, uri);
                return get_response(
                    StatusCode::FORBIDDEN,
                    FORBIDDEN,
                    &labels,
                    &start_time,
                    &req_size,
                );
            }
            Some(authorization) => authorization,
        },
        Some(authorization) => match authorization.to_str() {
            Err(e) => {
                warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='{}'", method_str, path, uri, format!("Error in authorization: {:#?}", e));
                return get_response(
                    StatusCode::FORBIDDEN,
                    FORBIDDEN,
                    &labels,
                    &start_time,
                    &req_size,
                );
            }
            Ok(authorization) => authorization.to_string(),
        },
    };
    let (claims, token_type) = match get_claims(&authorization).await {
        Some(claims) => claims,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='403' user_sub='Not yet decoded' token_id='Not yet decoded' error='Invalid or no claim'", method_str, path, uri);
            return get_response(
                StatusCode::FORBIDDEN,
                FORBIDDEN,
                &labels,
                &start_time,
                &req_size,
            );
        }
    };

    let forwarded_uri = match req.uri().path_and_query().map(|x| &x.as_str()[app.len()..]) {
        Some(forwarded_uri) => forwarded_uri,
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='Not yet decoded' token_id='Not yet decoded' error='Forward api not found'", method_str, path, uri);
            return get_response(
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &labels,
                &start_time,
                &req_size,
            );
        }
    };

    let forwarded_path = &req.uri().path()[app.len()..];

    match api_lock.read().await.get(app) {
        None => {
            warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='{}' token_id='{}' error='Forward api not found'", method_str, path, uri, claims.sub, claims.token_id);
            get_response(
                StatusCode::NOT_FOUND,
                NOT_FOUND,
                &labels,
                &start_time,
                &req_size,
            )
        }
        Some((api, node)) => match api.spec.mode {
            ApiMode::ForwardAll => {
                let endpoint = Endpoint::from_forward_all(
                    forwarded_path.to_string(),
                    method_str.to_string(),
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
                    &labels,
                    &start_time,
                    &req_size,
                    &http_uri_string,
                    &ws_uri_string,
                    &token_type,
                    method_str,
                )
                .await
            }
            ApiMode::ForwardStrict(_) => match node.match_path(forwarded_path, method_str) {
                None => {
                    warn!("method='{}' path='{}' uri='{}' status_code='404' user_sub='{}' token_id='{}' error='Endpoint not found in service'", method_str, path, uri, claims.sub, claims.token_id);
                    get_response(
                        StatusCode::NOT_FOUND,
                        NOT_FOUND,
                        &labels,
                        &start_time,
                        &req_size,
                    )
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
                        &labels,
                        &start_time,
                        &req_size,
                        &http_uri_string,
                        &ws_uri_string,
                        &token_type,
                        method_str,
                    )
                    .await
                }
            },
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    if let Err(e) = init_runtime_config() {
        error!("event='Runtime config is not valid: {}'", e);
        exit(1);
    };
    init_token_sources();

    let addr: SocketAddr = match RUNTIME_CONFIG.get().unwrap().bind_to.parse() {
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
        RUNTIME_CONFIG.get().unwrap().crd_label.to_owned(),
    );

    // Share a `Client` with all `Service`s
    let client = Client::new();

    let make_service = make_service_fn(move |_| {
        // Move a clone of `client`, `perm_lock` and `role_lock` into the `make_service`.
        let client = client.clone();
        let perm_lock = perm_lock.clone();
        let role_lock = role_lock.clone();
        let api_lock = api_lock.clone();
        async {
            Ok::<_, GenericError>(service_fn(move |req| {
                // Clone again to ensure that `client`, `perm_lock` and `role_lock` outlives this closure.
                response(
                    req,
                    client.to_owned(),
                    perm_lock.clone(),
                    role_lock.clone(),
                    api_lock.clone(),
                )
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_service);
    info!("event='Listening on http://{}'", addr);

    let res = tokio::try_join!(update_perm, update_api, async {
        server.await.map_err(|e| anyhow!(e))
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
