use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::vec::Vec;
use tokio::sync::RwLock;

use anyhow::{bail, Result};

use crate::api::ApiDefinition;
use crate::route::Node;
use crate::RUNTIME_CONFIG;

pub async fn update_api(
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
    _label_filter: String,
) -> Result<()> {
    let apidefinitions_file =
        fs::read_to_string(RUNTIME_CONFIG.apidefinition_path.clone().as_str())
            .expect("Failed reading apidefinitions file");
    let apidefinitions: Vec<ApiDefinition> = serde_yaml::from_str(apidefinitions_file.as_str())
        .expect("Failed deserializing apidefinitions");


    for apidefinition in apidefinitions.iter() {
        match apidefinition.check_fields() {
            Err(e) => {
                let err_msg = format!("Invalid apidefinition: {}", e);
                error!("event='{}'", err_msg);
                bail!(err_msg);
            }
            Ok(_) => {
                let node = Node::new(&apidefinition);
                let mut api_write = api_lock.write().await;
                let mut built_apidefinition = apidefinition.clone();
                built_apidefinition.build_uri();
                api_write.insert(
                    built_apidefinition.spec.app_name.clone(),
                    (built_apidefinition, node),
                );
                info!(
                    "event='{} api updated from {:?}'",
                    &apidefinition.spec.app_name,
                    &apidefinition
                        .metadata
                        .name
                        .as_ref()
                        .unwrap_or(&"NO_NAME_DEFINED".to_owned())
                );
            }

        }
    }
    Ok(())
}
