//! AKS (managed Kubernetes) primitives: list clusters and fetch a
//! kubeconfig, both via `az aks ...`.
//!
//! AKS is not a distinct transport in Oryxis: a cluster is reached the
//! same way any Kubernetes cluster is, through the `oryxis-cloud-k8s`
//! provider driving `kubectl`. The only Azure-specific step is turning a
//! cluster into a kubeconfig context, which `get_credentials` does. These
//! stay pure CLI helpers here so the app wiring composes them without this
//! crate owning how an AKS cluster surfaces in the UI.

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredAksCluster};

use crate::{run_az, AzureConfig};

/// One AKS cluster as emitted by `az aks list --output json`. Only the
/// fields the UI needs are declared.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AksCluster {
    /// Cluster name (the `--name` handle `get-credentials` takes).
    pub name: String,
    /// Resource group (the `--resource-group` handle `get-credentials`
    /// takes). az emits it as `resourceGroup`.
    #[serde(rename = "resourceGroup", default)]
    pub resource_group: String,
    /// Azure region.
    #[serde(default)]
    pub location: String,
    /// Runtime power state (`Running` / `Stopped`), az nests it under
    /// `powerState.code`.
    #[serde(rename = "powerState", default)]
    pub power_state: Option<AksPowerState>,
    /// `Succeeded` / `Creating` / `Failed`, the control-plane lifecycle.
    /// Used as the status when no `powerState` is present.
    #[serde(rename = "provisioningState", default)]
    pub provisioning_state: String,
    /// Agent pools; node count is the sum of their `count`s.
    #[serde(rename = "agentPoolProfiles", default)]
    pub agent_pool_profiles: Vec<AksAgentPool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AksPowerState {
    #[serde(default)]
    pub code: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AksAgentPool {
    #[serde(default)]
    pub count: u32,
}

impl AksCluster {
    /// The status the UI shows: the runtime power state when present
    /// (`Running` / `Stopped`), otherwise the provisioning state.
    fn status(&self) -> String {
        match &self.power_state {
            Some(p) if !p.code.trim().is_empty() => p.code.clone(),
            _ => self.provisioning_state.clone(),
        }
    }

    /// Total node count across the cluster's agent pools.
    fn node_count(&self) -> u32 {
        self.agent_pool_profiles.iter().map(|p| p.count).sum()
    }
}

/// The kubeconfig context name Oryxis asks `az aks get-credentials` to
/// write for a cluster: `<cluster>.<resource_group>`. az's own default is
/// the bare cluster name, which collides when two resource groups carry
/// same-named clusters (the dup-check would grey the second one forever,
/// and `--overwrite-existing` would silently repoint the first cluster's
/// context), so we pass an explicit `--context` compounding the resource
/// group, mirroring how GKE compounds project + location. The separator
/// is `.` on purpose: an AKS cluster name is restricted to letters,
/// digits, `-` and `_` (no dot), so the first `.` unambiguously splits
/// cluster from resource group even though both fields may themselves
/// contain `-` and `_`. A `-` separator was ambiguous (`app-prod` in rg
/// `eastus` and `app` in rg `prod-eastus` both yielded `app-prod-eastus`
/// and collided). No live resolution is needed, the name is
/// deterministic. A helper so the dup-check and the credential fetch
/// agree by construction. A blank resource group (never emitted by
/// `az aks list`, but tolerated) falls back to the bare cluster name
/// rather than minting a trailing dot.
pub fn context_name(cluster: &str, resource_group: &str) -> String {
    if resource_group.trim().is_empty() {
        cluster.to_string()
    } else {
        format!("{cluster}.{resource_group}")
    }
}

/// Parse `az aks list --output json` output. Pure, so it is unit-tested
/// against fixture JSON.
fn parse_clusters(json: &[u8]) -> Result<Vec<AksCluster>, CloudError> {
    serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing az aks JSON: {e}")))
}

/// List every AKS cluster visible to the profile's subscription.
pub async fn list_clusters(cfg: &AzureConfig) -> Result<Vec<AksCluster>, CloudError> {
    let out = run_az(cfg, &["aks", "list", "--output", "json"]).await?;
    parse_clusters(&out)
}

/// Best-effort AKS cluster discovery for the combined `discover()` pass:
/// map clusters onto the shared [`DiscoveredAksCluster`] shape. AKS is
/// independent of the VM API (a subscription may permit one but not the
/// other), so a listing failure (no permission, provider not registered)
/// yields an empty list rather than failing the whole discovery, which
/// would also hide the VMs. The user simply sees no AKS section.
pub async fn discover_clusters(
    cfg: &AzureConfig,
) -> Result<Vec<DiscoveredAksCluster>, CloudError> {
    let clusters = match list_clusters(cfg).await {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(clusters
        .into_iter()
        .map(|c| DiscoveredAksCluster {
            context: context_name(&c.name, &c.resource_group),
            status: c.status(),
            node_count: c.node_count(),
            name: c.name,
            resource_group: c.resource_group,
            location: c.location,
        })
        .collect())
}

/// Fetch (merge) a cluster's credentials into the active kubeconfig via
/// `az aks get-credentials`, and return the context name az wrote. After
/// this the `oryxis-cloud-k8s` provider can discover workloads and open
/// shells against `--context <name>`.
///
/// The context is named explicitly (`--context`, see [`context_name`]) so
/// same-named clusters in different resource groups get distinct contexts
/// instead of az's colliding bare-cluster-name default.
/// `--overwrite-existing` makes a re-add idempotent (az otherwise refuses
/// to clobber a same-named context); with the composite name it can only
/// ever overwrite this exact cluster's own context, and every fetch
/// re-establishes the same credentials, so overwriting is safe.
pub async fn get_credentials(
    cfg: &AzureConfig,
    cluster: &str,
    resource_group: &str,
) -> Result<String, CloudError> {
    let context = context_name(cluster, resource_group);
    // get-credentials writes to the kubeconfig; there is no useful stdout.
    // Success is the exit code.
    run_az(
        cfg,
        &[
            "aks",
            "get-credentials",
            "--name",
            cluster,
            "--resource-group",
            resource_group,
            "--context",
            &context,
            "--overwrite-existing",
        ],
    )
    .await?;
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_name_compounds_cluster_and_resource_group() {
        assert_eq!(context_name("prod", "rg-prod"), "prod.rg-prod");
        // Same cluster name in two resource groups must not collide.
        assert_ne!(context_name("prod", "rg-a"), context_name("prod", "rg-b"));
        // The `.` separator resolves the hyphen-boundary ambiguity that a
        // `-` separator had: these two distinct clusters used to both
        // render `app-prod-eastus`; now they stay distinct.
        assert_ne!(
            context_name("app-prod", "eastus"),
            context_name("app", "prod-eastus")
        );
        // A blank resource group falls back to the bare cluster name.
        assert_eq!(context_name("prod", ""), "prod");
        assert_eq!(context_name("prod", "  "), "prod");
    }

    #[test]
    fn parses_a_cluster_list() {
        let json = br#"[
          {
            "name": "prod",
            "resourceGroup": "rg-prod",
            "location": "eastus",
            "powerState": { "code": "Running" },
            "provisioningState": "Succeeded",
            "agentPoolProfiles": [ { "count": 3 }, { "count": 2 } ]
          },
          {
            "name": "dev",
            "resourceGroup": "rg-dev",
            "location": "westeurope",
            "provisioningState": "Creating",
            "agentPoolProfiles": []
          }
        ]"#;
        let clusters = parse_clusters(json).unwrap();
        assert_eq!(clusters.len(), 2);

        assert_eq!(clusters[0].name, "prod");
        assert_eq!(clusters[0].resource_group, "rg-prod");
        assert_eq!(clusters[0].status(), "Running");
        assert_eq!(clusters[0].node_count(), 5);

        // No powerState -> status falls back to provisioningState; no
        // agent pools -> zero nodes.
        assert_eq!(clusters[1].status(), "Creating");
        assert_eq!(clusters[1].node_count(), 0);
    }

    #[test]
    fn empty_cluster_list_is_ok() {
        assert!(parse_clusters(b"[]").unwrap().is_empty());
    }
}
