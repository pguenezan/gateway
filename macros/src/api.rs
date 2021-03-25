use std::collections::HashMap;

use anyhow::{anyhow, bail};
use serde::Deserialize;
use url::Url;

use crate::endpoint::Endpoint;

#[derive(Deserialize, Debug)]
#[serde(rename_all(deserialize = "snake_case"))]
#[serde(tag = "kind", content = "endpoints")]
pub enum ApiMode {
    ForwardAll,
    ForwardStrict(Vec<Endpoint>),
}

#[derive(Deserialize, Debug)]
pub struct Api {
    pub app_name: String,
    pub host: String,
    pub mode: ApiMode,
    pub forward_path: String,
}

impl Api {
    fn check_fields(&self) -> anyhow::Result<()> {
        self.check_app_name()?;
        self.check_host()?;
        self.check_endpoints()?;
        self.check_forward_path()?;

        Ok(())
    }

    fn check_app_name(&self) -> anyhow::Result<()> {
        if self.app_name.len() < 2 {
            bail!("app_name: {} must be at least 2 characters", self.app_name);
        }
        if !self.app_name.starts_with('/') {
            bail!("app_name: {} should start with `/`", self.app_name);
        }
        if self.app_name[1..].contains('/') {
            bail!("app_name: {} should only have one `/`", self.app_name);
        }
        if self.app_name == "/metrics" || self.app_name == "/health" {
            bail!(
                "app_name: {} cannot be `/metrics` or `/health`",
                self.app_name
            );
        }

        Ok(())
    }

    fn check_host(&self) -> anyhow::Result<()> {
        Url::parse(&format!("http://{}", self.host))
            .map_err(|_| anyhow!("host: {} isn't valid", self.host))
            .map(|_| ())
    }

    fn check_forward_path(&self) -> anyhow::Result<()> {
        if self.forward_path.is_empty() || self.forward_path.starts_with('/') {
            Ok(())
        } else {
            bail!("forward_path: {} should start with `/`", self.forward_path);
        }
    }

    fn check_endpoints(&self) -> anyhow::Result<()> {
        if let ApiMode::ForwardStrict(endpoints) = &self.mode {
            for endpoint in endpoints {
                endpoint.check_fields()?;
            }
        }

        Ok(())
    }
}

pub fn parse_apis(yaml_content: &str) -> anyhow::Result<Vec<Api>> {
    let apis: Vec<Api> = serde_yaml::from_str(yaml_content)?;

    for api in &apis {
        api.check_fields()?;
    }

    Ok(apis)
}
