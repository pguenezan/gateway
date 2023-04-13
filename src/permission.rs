use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Deserialize;

use bytes::Buf as _;

use regex::Regex;

use hyper::Client;

use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

use anyhow::bail;

use crate::runtime_config::{PermUri, RUNTIME_CONFIG};

use anyhow::Result;

#[derive(Deserialize, Debug)]
struct Perm {
    role_name: String,
    user_id: HashSet<String>,
}

type PermList = Vec<Perm>;

async fn fetch_perm(perm_uri: &PermUri) -> Option<PermList> {
    let client = Client::new();
    let res = match client.get(perm_uri.uri.clone()).await {
        Ok(res) => res,
        Err(e) => {
            error!("fail to fetch {:?}: {}", perm_uri, e);
            return None;
        }
    };
    let body = match hyper::body::aggregate(res).await {
        Ok(body) => body,
        Err(e) => {
            error!("fail to fetch {:?}: {}", perm_uri, e);
            return None;
        }
    };
    match serde_json::from_reader(body.reader()) {
        Ok(json) => json,
        Err(e) => {
            error!("fail to fetch {:?}: {}", perm_uri, e);
            None
        }
    }
}

pub async fn get_perm() -> Result<(
    HashMap<String, HashSet<String>>,
    HashMap<String, HashMap<String, String>>,
)> {
    let mut perm_hm: HashMap<String, HashSet<String>> = HashMap::new();
    let is_role_perm = Regex::new("([^:]+)::roles::(.*)").unwrap();
    let mut user_role = HashMap::new();

    for perm_uri in RUNTIME_CONFIG.perm_uris.iter().as_ref() {
        match fetch_perm(perm_uri).await {
            Some(perm_vec) => {
                for perm in perm_vec.iter() {
                    if is_role_perm.is_match(&perm.role_name) {
                        let captures = is_role_perm.captures(&perm.role_name).unwrap();
                        let app_name = captures.get(1).unwrap().as_str();
                        let role_name = captures.get(2).unwrap().as_str();
                        for user_id in perm.user_id.iter() {
                            user_role
                                .entry(user_id.to_string())
                                .or_insert_with(HashMap::new)
                                .entry(app_name.to_string())
                                .or_insert_with(Vec::new)
                                .push(role_name.to_string());
                        }
                    }
                    if perm_hm.contains_key(&perm.role_name) {
                        let old_value = perm_hm.get(&perm.role_name).unwrap();
                        let new_value: HashSet<String> = old_value
                            .union(&perm.user_id)
                            .map(|s| s.to_string())
                            .collect();
                        perm_hm.insert(perm.role_name.to_string(), new_value);
                    } else {
                        perm_hm.insert(perm.role_name.to_string(), perm.user_id.clone());
                    }
                }
            }
            None => {
                bail!("Fail to fetch permissions");
            }
        }
    }

    let mut user_role_final = HashMap::new();
    for (user_sub, apps) in &user_role {
        for (app_name, perms) in apps {
            let perm_str = perms
                .iter()
                .fold(String::new(), |acc, perm| acc + "," + perm);
            user_role_final
                .entry(user_sub.to_string())
                .or_insert_with(HashMap::new)
                .insert(app_name.to_string(), perm_str[1..].to_string());
        }
    }
    Ok((perm_hm, user_role_final))
}

pub async fn update_perm(
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    role_lock: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
) -> Result<()> {
    let mut error_count = 0;
    let max_fetch_error_count = RUNTIME_CONFIG.max_fetch_error_count;

    loop {
        sleep(Duration::from_millis(RUNTIME_CONFIG.perm_update_delay) * 1000).await;
        let perm_update = get_perm().await;
        if perm_update.is_err() {
            error_count += 1;
            error!(
                "Failed to fetch/update permissions for the {} times",
                error_count
            );

            if error_count >= max_fetch_error_count {
                bail!("Failed to fetch/update permissions")
            }
        } else {
            let (perm, role) = perm_update.unwrap();

            let mut perm_write = perm_lock.write().await;
            *perm_write = perm;
            drop(perm_write);

            let mut role_write = role_lock.write().await;
            *role_write = role;
            drop(role_write);

            error_count = 0;
            debug!("perm updated");
        }
    }
}

pub async fn has_perm(
    perm_lock: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    perm: &str,
    token_id: &str,
) -> bool {
    matches!(perm_lock.read().await.get(perm), Some(users) if users.contains(token_id))
}
