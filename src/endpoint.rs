use std::str::FromStr;

use lazy_static::lazy_static;

use schemars::JsonSchema;

use hyper::Method;
use regex::Regex;
use serde::{Deserialize, Serialize};

lazy_static! {
    static ref PATH_TO_PERM: Regex = Regex::new("\\{[^/]*\\}").unwrap();
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct Endpoint {
    pub path: String,
    pub method: String,
    #[serde(skip)]
    pub permission: String,
}

impl Endpoint {
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
                return Err(format!(
                    "param: `{}` contains `{{` or `}}` in path `{}`",
                    content, path
                ));
            }
            let preceded = captures.get(1).unwrap().as_str();
            let succeed = captures.get(3).unwrap().as_str();
            if preceded != "/" { //|| succeed != "/" {
                return Err(format!(
                    "param: `{}` must be preceded and succeed by `/` not (`{}`, `{}`) in path `{}`",
                    content, preceded, succeed, path
                ));
            }
            mut_path = match_param
                .replace(&format!("{{{}}}", &content), "")
                .to_string();
        }
        if mut_path.contains('{') || mut_path.contains('}') {
            return Err(format!("path: `{}` contains/is missing `{{` or `}}`", path));
        }
        Ok(())
    }

    fn check_path(&self) -> Result<(), String> {
        if self.path.is_empty() {
            return Err(format!("path: {} must be at least 1 characters", self.path));
        }
        if !self.path.starts_with('/') {
            return Err(format!("path: {} should start with `/`", self.path));
        }
        // if !self.path.ends_with('/') {
        //     return Err(format!("path: {} should end with `/`", self.path));
        // }

        Ok(())
    }

    fn check_method(&self) -> Result<(), String> {
        Method::from_str(&self.method)
            .map(|_| ())
            .map_err(|err| format!("couldn't parse method: {}", err))
    }
}
