use std::str::FromStr;

use hyper::Method;
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

static PATH_TO_PERM: Lazy<Regex> = Lazy::new(|| Regex::new("\\{[^/]*\\}").unwrap());

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct Endpoint {
    pub path: String,
    pub method: String,
    #[serde(default = "is_websocket_default")]
    pub is_websocket: bool,
    #[serde(skip)]
    pub permission: String,
    #[serde(default = "check_permission_default")]
    pub check_permission: bool,
}

fn is_websocket_default() -> bool {
    false
}

fn check_permission_default() -> bool {
    false
}

impl Endpoint {
    pub(crate) fn from_forward_all(path: String, method: String, app: &str) -> Self {
        Self {
            permission: format!("{}::{}::FULL_ACCESS", &app[1..], &method),
            path,
            method,
            is_websocket: false,
            check_permission: true,
        }
    }
    pub(crate) fn check_fields(&self) -> Result<(), String> {
        self.check_path()?;
        self.check_parameters()?;
        self.check_method()?;

        Ok(())
    }

    pub fn build_permission(&mut self, app: &str) {
        self.permission = format!(
            "{}::{}::{}",
            app,
            self.method,
            PATH_TO_PERM.replace_all(&self.path, "{}")
        );
    }

    fn check_parameters(&self) -> Result<(), String> {
        let path = &self.path;

        let match_param = Regex::new("(.?)\\{([^/]+)\\}(.?)").unwrap();
        let mut mut_path: String = path.to_string();
        while match_param.is_match(&mut_path) {
            let captures = match_param.captures(&mut_path).unwrap();
            let content = captures.get(2).unwrap().as_str();
            if content.contains('{') || content.contains('}') {
                let err_msg = format!(
                    "param: `{}` contains `{{` or `}}` in path `{}`",
                    content, path
                );
                info!("event='{}'", err_msg);
                return Err(err_msg);
            }
            let preceded = captures.get(1).unwrap().as_str();
            let succeed = captures.get(3).unwrap().as_str();
            if preceded != "/" {
                let err_msg = format!(
                    "param: `{}` must be preceded and succeed by `/` not (`{}`, `{}`) in path `{}`",
                    content, preceded, succeed, path
                );
                info!("event='{}'", err_msg);
                return Err(err_msg);
            }
            mut_path = match_param
                .replace(&format!("{{{}}}", &content), "")
                .to_string();
        }
        Ok(())
    }

    fn check_path(&self) -> Result<(), String> {
        if self.path.is_empty() {
            let err_msg = format!("path: {} must be at least 1 characters", self.path);
            info!("event='{}'", err_msg);
            return Err(err_msg);
        }
        if !self.path.starts_with('/') {
            let err_msg = format!("path: {} should start with `/`", self.path);
            info!("event='{}'", err_msg);
            return Err(err_msg);
        }

        Ok(())
    }

    fn check_method(&self) -> Result<(), String> {
        match Method::from_str(&self.method)
            .map(|_| ())
            .map_err(|err| format!("couldn't parse method: {}", err))
        {
            Ok(_) => Ok(()),
            Err(err_msg) => {
                info!("event='{}'", err_msg);
                Err(err_msg)
            }
        }
    }
}
