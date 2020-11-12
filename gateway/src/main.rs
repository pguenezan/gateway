use std::env;
use std::net::SocketAddr;
use std::process::exit;

use hyper::client::HttpConnector;
use hyper::header::HeaderValue;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, HeaderMap, Request, Response, Server, StatusCode};

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

fn inject_headers(headers: &mut HeaderMap<HeaderValue>, claims: &Claims) {
    if let Ok(value) = claims.sub.parse() {
        headers.insert("X-Forwarded-User", value);
    };
}

async fn response(mut req: Request<Body>, client: Client<HttpConnector>) -> Result<Response<Body>> {
    let claims = match get_claims(req.headers()).await {
        Some(claims) => claims,
        None => return get_response!(StatusCode::FORBIDDEN, FORBIDDEN),
    };

    let path = &req.uri().path();
    let slash_index = match path[1..].find('/') {
        Some(slash_index) => slash_index + 1,
        None => return get_response!(StatusCode::NOT_FOUND, NOTFOUND),
    };
    let app = &path[..slash_index];

    println!("uri = {}", req.uri());
    println!("app = {}", app);

    let forwarded_path = match req
        .uri()
        .path_and_query()
        .map(|x| &x.as_str()[app.len() + 1..])
    {
        Some(forwarded_path) => forwarded_path,
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
