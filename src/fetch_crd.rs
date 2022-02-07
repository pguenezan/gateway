use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use kube::api::{Api, ListParams};
use kube::Client;

use futures::{StreamExt, TryStreamExt};
use kube_runtime::{utils::try_flatten_applied, watcher};

use crate::api::ApiDefinition;
use crate::route::Node;

pub async fn update_api(
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
    label_filter: String,
) {
    let client = match Client::try_default().await {
        Ok(client) => client,
        Err(e) => {
            error!("kube client: {:?}", e);
            return;
        }
    };
    let apidefinitions: Api<ApiDefinition> = Api::all(client);
    let lp = ListParams::default().labels(&label_filter);
    let watcher = watcher(apidefinitions, lp);

    let mut apply_apidefinitions = try_flatten_applied(watcher).boxed_local();
    loop {
        match apply_apidefinitions.try_next().await {
            Err(e) => {
                error!("crd stream: {:?}", e);
                return;
            }
            Ok(None) => {
                error!("missing apidefinition");
                return;
            }
            Ok(Some(ref apidefinition)) => match apidefinition.check_fields() {
                Err(e) => {
                    error!("invalid apidefinition {}", e);
                    return;
                }
                Ok(_) => {
                    let node = Node::new(apidefinition);
                    let mut api_write = api_lock.write().await;
                    let mut built_apidefinition = apidefinition.clone();
                    built_apidefinition.build_uri();
                    api_write.insert(
                        built_apidefinition.spec.app_name.clone(),
                        (built_apidefinition, node),
                    );
                    println!(
                        "{} api updated from {:?}",
                        &apidefinition.spec.app_name,
                        &apidefinition
                            .metadata
                            .name
                            .as_ref()
                            .unwrap_or(&"NO_NAME_DEFINED".to_owned())
                    );
                }
            },
        };
    }
}
