use std::env;
use std::net::SocketAddr;
use std::process::exit;

use hyper::client::HttpConnector;
use hyper::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, HeaderMap, Request, Response, Server, StatusCode};

use lazy_static::lazy_static;

use prometheus::{
    opts, register_counter_vec, register_histogram_vec, CounterVec, Encoder, HistogramVec,
    TextEncoder,
};

mod auth;
use auth::{get_claims, Claims};

use macros::gateway_config;

type GenericError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, GenericError>;

static OK: &[u8] = b"Ok";
static NOTFOUND: &[u8] = b"Not Found";
static FORBIDDEN: &[u8] = b"Forbidden";
static BADGATEWAY: &[u8] = b"Bad Gateway";

macro_rules! get_response {
    ($status_code:expr, $content:expr) => {
        Ok(Response::builder()
            .status($status_code)
            .body($content.into())
            .unwrap())
    };
}

const LABEL_NAMES: [&str; 3] = ["app", "path", "method"];

lazy_static! {
    static ref HTTP_COUNTER: CounterVec = register_counter_vec!(
        opts!(
            "gateway_http_requests_total",
            "Number of HTTP requests made."
        ),
        &LABEL_NAMES
    )
    .unwrap();
    static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "gateway_http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &LABEL_NAMES
    )
    .unwrap();
}

fn inject_headers(
    headers: &mut HeaderMap<HeaderValue>,
    claims: &Claims,
    role_prefix: &str,
    token_type: &str,
) {
    if cfg!(feature = "remove_authorization_header") {
        headers.remove("Authorization");
    }
    if let Ok(value) = claims.sub.parse() {
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
    let roles = claims
        .roles
        .iter()
        .filter(|role| role.starts_with(role_prefix))
        .map(|role| &role[role_prefix.len()..])
        .collect::<Vec<&str>>()
        .join(",");
    if let Ok(value) = roles.parse() {
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

async fn response(mut req: Request<Body>, client: Client<HttpConnector>) -> Result<Response<Body>> {
    match req.uri().path() {
        "/metrics" => return metrics().await,
        "/health" => return get_response!(StatusCode::OK, OK),
        _ => (),
    }

    let path = &req.uri().path();
    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
    };
    let app = &path[..slash_index];

    let mut forwarded_path = &req.uri().path()[app.len()..];

    let authorization = match req.headers().get(AUTHORIZATION) {
        None => return get_response!(StatusCode::FORBIDDEN, FORBIDDEN),
        Some(authorization) => match authorization.to_str() {
            Err(_) => return get_response!(StatusCode::FORBIDDEN, FORBIDDEN),
            Ok(authorization) => authorization,
        },
    };
    let (claims, token_type) = match get_claims(authorization).await {
        Some(claims) => claims,
        None => return get_response!(StatusCode::FORBIDDEN, FORBIDDEN),
    };

    let forwarded_uri = match req
        .uri()
        .path_and_query()
        .map(|x| &x.as_str()[app.len() + 1..])
    {
        Some(forwarded_uri) => forwarded_uri,
        None => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
    };

    include!("config.rs")
}

#[tokio::main]
async fn main() -> Result<()> {
    let bind_to = match env::var("BIND_TO") {
        Ok(bind_to) => bind_to,
        Err(_) => {
            eprintln!("missing BIND_TO from environment");
            exit(1);
        }
    };

    let addr: SocketAddr = match bind_to.parse() {
        Ok(addr) => addr,
        Err(_) => {
            eprintln!("BIND_TO is not a valid");
            exit(1);
        }
    };

    // Share a `Client` with all `Service`s
    let client = Client::new();

    let make_service = make_service_fn(move |_| {
        // Move a clone of `client` into the `make_service`.
        let client = client.clone();
        async {
            Ok::<_, GenericError>(service_fn(move |req| {
                // Clone again to ensure that client outlives this closure.
                response(req, client.to_owned())
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_service);
    println!("Listening on http://{}", addr);

    server.await?;

    Ok(())
}
