use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{bail, Result};
use futures::{Stream, StreamExt, TryStreamExt};
use kube::api::{Api, ApiResource, DynamicObject};
use kube::core::GroupVersionKind;
use kube::{discovery, Client};
use kube_runtime::utils::WatchStreamExt;
use kube_runtime::watcher;
use kube_runtime::watcher::Config;
use tokio::sync::RwLock;

use crate::api::ApiDefinition;
use crate::route::Node;

async fn read_crds(
    mut stream: Pin<Box<dyn Stream<Item = Result<DynamicObject, watcher::Error>> + Send>>,
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
) -> Result<()> {
    loop {
        match stream.try_next().await {
            Err(e) => {
                let err_msg = format!("Crd stream: {:?}", e);
                error!("event='{}'", err_msg);
                bail!(err_msg);
            }
            Ok(None) => {
                info!("event='No apidefinition found'");
            }
            Ok(Some(ref apidefinition)) => match ApiDefinition::try_from(apidefinition) {
                Err(e) => {
                    let err_msg = format!(
                        "event='An error occurs during apidefinition parsing: {}'",
                        e
                    );
                    error!("event='{}'", err_msg);
                }
                Ok(apidefinition) => match apidefinition.check_fields() {
                    Err(e) => {
                        let err_msg = format!("Invalid apidefinition: {}", e);
                        error!("event='{}'", err_msg);
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
                },
            },
        };
    }
}

async fn update_api_namespaced(
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
    namespaces: Vec<String>,
    api_resource: ApiResource,
    client: Client,
    watcher_config: watcher::Config,
) -> Result<()> {
    for ns in namespaces {
        let apidefinitions =
            Api::<DynamicObject>::namespaced_with(client.clone(), ns.as_str(), &api_resource);
        let watcher = watcher(apidefinitions, watcher_config.clone());
        let apply_apidefinitions = watcher.applied_objects().boxed();
        read_crds(apply_apidefinitions, api_lock.clone()).await?;
    }

    Ok(())
}

async fn update_api_cluster(
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
    api_resource: ApiResource,
    client: Client,
    watcher_config: watcher::Config,
) -> Result<()> {
    let apidefinitions = Api::<DynamicObject>::all_with(client.clone(), &api_resource);
    let watcher = watcher(apidefinitions, watcher_config.clone());
    let apply_apidefinitions = watcher.applied_objects().boxed();
    read_crds(apply_apidefinitions, api_lock.clone()).await
}

pub async fn update_api(
    api_lock: Arc<RwLock<HashMap<String, (ApiDefinition, Node)>>>,
    label_filter: String,
    crds_namespace: Option<Vec<String>>,
) -> Result<()> {
    let client = match Client::try_default().await {
        Ok(client) => client,
        Err(e) => {
            let err_msg = format!("kube client: {:?}", e);
            error!("event='{}'", err_msg);
            bail!(err_msg);
        }
    };
    let group = "gateway.dgexsol.fr";
    let version = "v2";
    let kind = "ApiDefinition";

    let gvk = GroupVersionKind::gvk(group, version, kind);
    // Use API discovery to identify more information about the type (like its plural)
    let (ar, _caps) = discovery::pinned_kind(&client, &gvk).await?;

    let lp = Config::default().labels(&label_filter);

    match crds_namespace {
        Some(namespaces) => update_api_namespaced(api_lock, namespaces, ar, client, lp).await,
        None => update_api_cluster(api_lock, ar, client, lp).await,
    }
}
