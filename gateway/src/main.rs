use std::env;
use std::net::SocketAddr;
use std::process::exit;

use hyper::client::HttpConnector;
use hyper::header::{HeaderValue, CONTENT_TYPE};
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
        opts!("http_requests_total", "Number of HTTP requests made."),
        &LABEL_NAMES
    )
    .unwrap();
    static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &LABEL_NAMES
    )
    .unwrap();
}

fn inject_headers(headers: &mut HeaderMap<HeaderValue>, claims: &Claims) {
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
    if req.uri().path() == "/metrics" {
        return metrics().await;
    }

    let path = &req.uri().path();
    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
    };
    let app = &path[..slash_index];

    let forwarded_path = &req.uri().path()[app.len()..];
    let method_str: &str = &req.method().to_string();
    let perm = format!("{}::{}::{}", &app[1..], method_str, forwarded_path);

    let labels = [&app[1..], forwarded_path, method_str];
    HTTP_COUNTER.with_label_values(&labels).inc();
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&labels).start_timer();

    let claims = match get_claims(req.headers()).await {
        Some(claims) => claims,
        None => return get_response!(StatusCode::FORBIDDEN, FORBIDDEN),
    };
    if !claims.roles.contains(&perm) {
        return get_response!(StatusCode::FORBIDDEN, FORBIDDEN);
    }

    println!("{} ({}) => {}", claims.preferred_username, claims.sub, perm);

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
