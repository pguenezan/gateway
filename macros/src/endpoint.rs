use std::str::FromStr;

use anyhow::{anyhow, bail};
use hyper::Method;
use regex::Regex;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Endpoint {
    pub path: String,
    pub method: String,
}

impl Endpoint {
    pub(crate) fn check_fields(&self) -> anyhow::Result<()> {
        self.check_path()?;
        self.check_parameters()?;
        self.check_method()?;

        Ok(())
    }

    fn check_parameters(&self) -> anyhow::Result<()> {
        let path = &self.path;

        let match_param = Regex::new("(.?)\\{([^/]+)\\}(.?)").unwrap();
        let mut mut_path: String = path.to_string();
        while match_param.is_match(&mut_path) {
            let captures = match_param.captures(&mut_path).unwrap();
            let content = captures.get(2).unwrap().as_str();
            if content.contains('{') || content.contains('}') {
                bail!(
                    "param: `{}` contains `{{` or `}}` in path `{}`",
                    content,
                    path
                );
            }
            let preceded = captures.get(1).unwrap().as_str();
            let succeed = captures.get(3).unwrap().as_str();
            if preceded != "/" || succeed != "/" {
                bail!(
                    "param: `{}` must be preceded and succeed by `/` not (`{}`, `{}`) in path `{}`",
                    content,
                    preceded,
                    succeed,
                    path
                );
            }
            mut_path = match_param
                .replace(&format!("{{{}}}", &content), "")
                .to_string();
        }
        if mut_path.contains('{') || mut_path.contains('}') {
            bail!("path: `{}` contains/is missing `{{` or `}}`", path);
        }
        Ok(())
    }

    fn check_path(&self) -> anyhow::Result<()> {
        if self.path.is_empty() {
            bail!("path: {} must be at least 1 characters", self.path);
        }
        if !self.path.starts_with('/') {
            bail!("path: {} should start with `/`", self.path);
        }
        if !self.path.ends_with('/') {
            bail!("path: {} should end with `/`", self.path);
        }

        Ok(())
    }

    fn check_method(&self) -> anyhow::Result<()> {
        Method::from_str(&self.method)
            .map(|_| ())
            .map_err(|err| anyhow!("couldn't parse method: {}", err))
    }
}
