use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use kube::api::{Api, ListParams};
use kube::Client;

use futures::{StreamExt, TryStreamExt};
use kube_runtime::{utils::try_flatten_applied, watcher};

use crate::api::ApiDefinition;
use crate::route::Node;

pub async fn update_api(api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>) {
    let client = match Client::try_default().await {
        Ok(client) => client,
        Err(e) => {
            error!("{}", e);
            return;
        }
    };
    let apidefinitions: Api<ApiDefinition> = Api::all(client);
    let lp = ListParams::default().labels("gateway/target=dev");
    let watcher = watcher(apidefinitions, lp);

    let mut apply_apidefinitions = try_flatten_applied(watcher).boxed_local();
    loop {
        match apply_apidefinitions.try_next().await {
            Err(e) => {
                error!("{}", e);
                return;
            }
            Ok(None) => {
                error!("missing apidefinition");
                return;
            }
            Ok(Some(ref apidefinition)) => match apidefinition.check_fields() {
                Err(e) => {
                    error!("{}", e);
                    return;
                }
                Ok(_) => {
                    let node = Node::new(apidefinition);
                    let mut api_write = api_lock.write().await;
                    let mut built_appdefinition = apidefinition.clone();
                    built_appdefinition.build_uri();
                    api_write.insert(
                        built_appdefinition.spec.app_name.clone(),
                        (built_appdefinition, node),
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
