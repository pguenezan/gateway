use serde::{Deserialize, Serialize};
use url::Url;

use kube::CustomResource;
use schemars::JsonSchema;

use crate::endpoint::Endpoint;
use anyhow::Result;

#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
#[serde(rename_all(deserialize = "snake_case"))]
#[serde(tag = "kind", content = "endpoints")]
pub enum ApiMode {
    ForwardAll,
    ForwardStrict(Vec<Endpoint>),
}

#[derive(CustomResource, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[kube(
    group = "gateway.dgexsol.fr",
    version = "v1",
    kind = "ApiDefinition",
    namespaced
)]
pub struct ApiDefinitionSpec {
    pub app_name: String,
    pub host: String,
    pub mode: ApiMode,
    #[serde(default = "forward_path_default")]
    pub forward_path: String,
    #[serde(skip)]
    pub uri_http: String,
    #[serde(skip)]
    pub uri_ws: String,
}

fn forward_path_default() -> String {
    "".to_string()
}

impl ApiDefinition {
    pub fn check_fields(&self) -> Result<(), String> {
        self.check_app_name()?;
        self.check_host()?;
        self.check_endpoints()?;
        self.check_forward_path()?;

        Ok(())
    }

    pub fn build_uri(&mut self) {
        self.spec.uri_http = format!("http://{}{}", &self.spec.host, &self.spec.forward_path);
        self.spec.uri_ws = format!("ws://{}{}", &self.spec.host, &self.spec.forward_path);
    }

    fn check_app_name(&self) -> Result<(), String> {
        if self.spec.app_name.len() < 2 {
            let err_msg = format!(
                "app_name: {} must be at least 2 characters",
                self.spec.app_name
            );
            info!("event='{}", err_msg);
            return Err(err_msg);
        }
        if !self.spec.app_name.starts_with('/') {
            let err_msg = format!("app_name: {} should start with `/`", self.spec.app_name);
            info!("event='{}", err_msg);
            return Err(err_msg);
        }
        if self.spec.app_name[1..].contains('/') {
            let err_msg = format!("app_name: {} should only have one `/`", self.spec.app_name);
            info!("event='{}", err_msg);
            return Err(err_msg);
        }
        if self.spec.app_name == "/metrics" || self.spec.app_name == "/health" {
            let err_msg = format!(
                "app_name: {} cannot be `/metrics` or `/health`",
                self.spec.app_name
            );
            info!("event='{}", err_msg);
            return Err(err_msg);
        }

        Ok(())
    }

    fn check_host(&self) -> Result<(), String> {
        match Url::parse(&format!("http://{}", self.spec.host))
            .map(|_| ())
            .map_err(|_| format!("host: {} isn't valid", self.spec.host))
        {
            Ok(_) => Ok(()),
            Err(e) => {
                info!("event='{:?}", e);
                anyhow::Result::Err(e)
            }
        }
    }

    fn check_forward_path(&self) -> Result<(), String> {
        if self.spec.forward_path.is_empty() || self.spec.forward_path.starts_with('/') {
            return Ok(());
        }
        let err_msg = format!(
            "Forward_path: {} should start with `/`",
            self.spec.forward_path
        );
        info!("event='{}'", err_msg);
        Err(err_msg)
    }

    fn check_endpoints(&self) -> Result<(), String> {
        if let ApiMode::ForwardStrict(endpoints) = &self.spec.mode {
            for endpoint in endpoints {
                endpoint.check_fields()?;
            }
        }

        Ok(())
    }
}
