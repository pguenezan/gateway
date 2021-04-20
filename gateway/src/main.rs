use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::process::exit;
use std::time::Instant;

use std::sync::Arc;
use tokio::sync::RwLock;

use http_body::SizeHint;
use hyper::body::HttpBody;
use hyper::client::HttpConnector;
use hyper::header::{
    HeaderValue, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, AUTHORIZATION, CONTENT_TYPE,
};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, HeaderMap, Method, Request, Response, Server, StatusCode};

use lazy_static::lazy_static;

use prometheus::{
    exponential_buckets, opts, register_counter_vec, register_histogram_vec, CounterVec, Encoder,
    HistogramVec, TextEncoder,
};

#[macro_use]
extern crate log;

mod auth;
use auth::{get_claims, init_token_sources, Claims};

mod runtime_config;
use runtime_config::{init_runtime_config, RUNTIME_CONFIG};

mod permission;
use permission::{get_perm, update_perm};

use macros::gateway_config;

type GenericError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, GenericError>;

static OK: &[u8] = b"Ok";
static NOTFOUND: &[u8] = b"Not Found";
static FORBIDDEN: &[u8] = b"Forbidden";
static BADGATEWAY: &[u8] = b"Bad Gateway";
static NOCONTENT: &[u8] = b"";

#[inline(always)]
fn commit_metrics(
    labels: &[&str],
    start_time: &Instant,
    status_code: StatusCode,
    req_size: &SizeHint,
    res_size: &SizeHint,
) {
    let full_labels = vec![labels[0], labels[1], labels[2], status_code.as_str()];

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
        .body(content.into())?;

    commit_metrics(
        labels,
        start_time,
        status_code,
        req_size,
        &response.size_hint(),
    );

    Ok(response)
}

const LABEL_NAMES: [&str; 4] = ["app", "path", "method", "status_code"];

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
    };
    if let Ok(value) = claims.preferred_username.parse() {
        headers.insert("X-Forwarded-User-Username", value);
    };
    if let Ok(value) = claims.given_name.parse() {
        headers.insert("X-Forwarded-User-First-Name", value);
    };
    if let Ok(value) = claims.family_name.parse() {
        headers.insert("X-Forwarded-User-Last-Name", value);
    };
    if let Ok(value) = claims.email.parse() {
        headers.insert("X-Forwarded-User-Email", value);
    }
    if let Ok(value) = app_user_roles.parse() {
        headers.insert("X-Forwarded-User-Roles", value);
    }
    if let Ok(value) = token_type.parse() {
        headers.insert("X-Forwarded-User-Type", value);
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

async fn response(
    mut req: Request<Body>,
    client: Client<HttpConnector>,
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
) -> Result<Response<Body>> {
    match req.uri().path() {
        "/metrics" => {
            return metrics().await;
        }
        "/health" => {
            return health().await;
        }
        _ => (),
    };

    let start_time = Instant::now();

    let path = &req.uri().path().to_owned();
    let method_str: &str = &req.method().to_string();
    let req_size = req.size_hint();

    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => {
            return get_response(
                StatusCode::NOT_FOUND,
                &NOTFOUND,
                &["", path, method_str],
                &start_time,
                &req_size,
            );
        }
    };
    let app = &path[..slash_index];

    let labels = [app, path, method_str];

    // to handle CORS pre flights
    if req.method() == Method::OPTIONS {
        return get_response(
            StatusCode::NO_CONTENT,
            &NOCONTENT,
            &labels,
            &start_time,
            &req_size,
        );
    }

    let mut forwarded_path = &req.uri().path()[app.len()..];

    let authorization = match req.headers().get(AUTHORIZATION) {
        None => {
            debug!("no Authorization header");
            return get_response(
                StatusCode::FORBIDDEN,
                &FORBIDDEN,
                &labels,
                &start_time,
                &req_size,
            );
        }
        Some(authorization) => match authorization.to_str() {
            Err(e) => {
                debug!("error in authorization: {:#?}", e);
                return get_response(
                    StatusCode::FORBIDDEN,
                    &FORBIDDEN,
                    &labels,
                    &start_time,
                    &req_size,
                );
            }
            Ok(authorization) => authorization,
        },
    };
    let (claims, token_type) = match get_claims(authorization).await {
        Some(claims) => claims,
        None => {
            debug!("no or missing claimsin authorization");
            return get_response(
                StatusCode::FORBIDDEN,
                &FORBIDDEN,
                &labels,
                &start_time,
                &req_size,
            );
        }
    };

    let forwarded_uri = match req
        .uri()
        .path_and_query()
        .map(|x| &x.as_str()[app.len() + 1..])
    {
        Some(forwarded_uri) => forwarded_uri,
        None => {
            return get_response(
                StatusCode::NOT_FOUND,
                &NOTFOUND,
                &labels,
                &start_time,
                &req_size,
            );
        }
    };

    // HACK: inform the compiler that a build should trigger if config.yaml is modified
    const _: &str = include_str!("../config.yaml");

    gateway_config!("config.yaml")
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    if let Err(e) = init_runtime_config() {
        error!("runtime config is not valid: {}", e);
        exit(1);
    };
    init_token_sources();

    let addr: SocketAddr = match RUNTIME_CONFIG.get().unwrap().bind_to.parse() {
        Ok(addr) => addr,
        Err(_) => {
            error!("bind_to is not valid");
            exit(1);
        }
    };

    let (perm, role) = get_perm().await.unwrap();
    let perm_lock = Arc::new(RwLock::new(perm));
    let role_lock = Arc::new(RwLock::new(role));

    let update_perm = update_perm(perm_lock.clone(), role_lock.clone());

    // Share a `Client` with all `Service`s
    let client = Client::new();

    let make_service = make_service_fn(move |_| {
        // Move a clone of `client`, `perm_lock` and `role_lock` into the `make_service`.
        let client = client.clone();
        let perm_lock = perm_lock.clone();
        let role_lock = role_lock.clone();
        async {
            Ok::<_, GenericError>(service_fn(move |req| {
                // Clone again to ensure that `client`, `perm_lock` and `role_lock` outlives this closure.
                response(req, client.to_owned(), perm_lock.clone(), role_lock.clone())
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_service);
    info!("Listening on http://{}", addr);

    tokio::join!(
        async {
            if let Err(e) = server.await {
                error!("fail to run server: {}", e);
            }
        },
        async {
            update_perm.await;
        },
    );

    Ok(())
}
