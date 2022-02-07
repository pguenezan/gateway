use std::env;
use std::error;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::process::exit;

use serde::Deserialize;

use once_cell::sync::OnceCell;

use hyper::http::Uri;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

#[derive(Debug, Deserialize)]
pub struct PermUri {
    #[serde(with = "http_serde::uri")]
    pub uri: Uri,
}

#[derive(Debug, Deserialize)]
pub struct AuthSource {
    pub name: String,
    pub token_type: String,
    pub issuer: String,
    pub audience: String,
    pub public_key: String,
}

#[derive(Debug, Deserialize)]
struct WebSocketConfigInternal {
    max_send_queue: usize,
    max_message_size: usize,
    max_frame_size: usize,
    accept_unmasked_frames: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeConfig {
    pub bind_to: String,
    pub crd_label: String,
    pub metrics_prefix: String,
    pub perm_uris: Vec<PermUri>,
    pub perm_update_delay: u64,
    pub auth_sources: Vec<AuthSource>,
    pub max_fetch_error: u64,
    websocket_config: WebSocketConfigInternal,
}

type Result<T> = std::result::Result<T, Box<dyn error::Error>>;

pub static RUNTIME_CONFIG: OnceCell<RuntimeConfig> = OnceCell::new();

fn get_runtime_config<P: AsRef<Path>>(path: P) -> Result<RuntimeConfig> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let runtime_config = serde_yaml::from_reader(reader)?;
    Ok(runtime_config)
}

pub fn init_runtime_config() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        error!("usage: {} runtime_config.yaml", args.first().unwrap());
        exit(1);
    }
    let path = Path::new(args.get(1).unwrap());
    RUNTIME_CONFIG.set(get_runtime_config(path)?).unwrap();
    Ok(())
}

impl RuntimeConfig {
    pub fn get_websocket_config(&self) -> WebSocketConfig {
        WebSocketConfig {
            max_send_queue: Some(self.websocket_config.max_send_queue),
            max_message_size: Some(self.websocket_config.max_message_size),
            max_frame_size: Some(self.websocket_config.max_frame_size),
            accept_unmasked_frames: self.websocket_config.accept_unmasked_frames,
        }
    }
}
