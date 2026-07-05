//! GKE (managed Kubernetes) primitives: list clusters and fetch a
//! kubeconfig, both via `gcloud container clusters ...`.
//!
//! GKE is not a distinct transport in Oryxis: a cluster is reached the
//! same way any Kubernetes cluster is, through the `oryxis-cloud-k8s`
//! provider driving `kubectl`. The only GCP-specific step is turning a
//! cluster into a kubeconfig context, which `get_credentials` does. These
//! stay pure CLI helpers here so the eventual app wiring (which decides
//! how a GKE cluster surfaces in the UI) composes them without this crate
//! owning that decision.

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredGkeCluster};

use crate::{run_gcloud, GcpConfig};

/// One GKE cluster as emitted by `gcloud container clusters list
/// --format=json`. Only the fields the UI needs are declared.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GkeCluster {
    /// Cluster name (the handle `get-credentials` takes).
    pub name: String,
    /// Region or zone the cluster lives in (`--location` for
    /// `get-credentials`). gcloud emits it as `location`.
    #[serde(default)]
    pub location: String,
    /// `RUNNING`, `PROVISIONING`, ... Uppercased as gcloud emits it.
    #[serde(default)]
    pub status: String,
    /// Total node count across the cluster's node pools.
    #[serde(rename = "currentNodeCount", default)]
    pub node_count: u32,
}

/// The kubeconfig context name `gcloud container clusters get-credentials`
/// creates for a cluster: `gke_<project>_<location>_<name>`. When the
/// project is unknown (active-project profile), the segment is left blank,
/// matching gcloud, which still fills it from the resolved active project
/// at credential time; callers that need the exact name should read it
/// back from the kubeconfig instead of assuming.
pub fn context_name(project: Option<&str>, location: &str, cluster: &str) -> String {
    format!("gke_{}_{}_{}", project.unwrap_or(""), location, cluster)
}

/// Parse `gcloud container clusters list --format=json` output. Pure, so
/// it is unit-tested against fixture JSON.
fn parse_clusters(json: &[u8]) -> Result<Vec<GkeCluster>, CloudError> {
    serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing gcloud clusters JSON: {e}")))
}

/// List every GKE cluster visible to the profile.
pub async fn list_clusters(cfg: &GcpConfig) -> Result<Vec<GkeCluster>, CloudError> {
    let out = run_gcloud(
        cfg,
        &["container", "clusters", "list", "--format=json"],
    )
    .await?;
    parse_clusters(&out)
}

/// Best-effort GKE cluster discovery for the combined `discover()` pass:
/// map clusters onto the shared [`DiscoveredGkeCluster`] shape. GKE is
/// independent of Compute Engine (a project may enable one API but not
/// the other), so a listing failure (Container API off, no permission)
/// yields an empty list rather than failing the whole discovery, which
/// would also hide the Compute VMs. The user simply sees no GKE section.
pub async fn discover_clusters(cfg: &GcpConfig) -> Result<Vec<DiscoveredGkeCluster>, CloudError> {
    let clusters = match list_clusters(cfg).await {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    let project = cfg.project.as_deref();
    Ok(clusters
        .into_iter()
        .map(|c| DiscoveredGkeCluster {
            context: context_name(project, &c.location, &c.name),
            name: c.name,
            location: c.location,
            status: c.status,
            node_count: c.node_count,
        })
        .collect())
}

/// Fetch (merge) a cluster's credentials into the active kubeconfig via
/// `gcloud container clusters get-credentials`, and return the context
/// name gcloud wrote. After this the `oryxis-cloud-k8s` provider can
/// discover workloads and open shells against `--context <name>`.
///
/// `location` is the cluster's region or zone (from [`GkeCluster::location`]).
pub async fn get_credentials(
    cfg: &GcpConfig,
    cluster: &str,
    location: &str,
) -> Result<String, CloudError> {
    // get-credentials writes to the kubeconfig and prints progress to
    // stderr; there is no useful stdout. Success is the exit code.
    run_gcloud(
        cfg,
        &[
            "container",
            "clusters",
            "get-credentials",
            cluster,
            "--location",
            location,
        ],
    )
    .await?;
    Ok(context_name(cfg.project.as_deref(), location, cluster))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_name_matches_gcloud_shape() {
        assert_eq!(
            context_name(Some("my-proj"), "us-central1", "prod"),
            "gke_my-proj_us-central1_prod"
        );
        // Active-project profile: blank project segment.
        assert_eq!(context_name(None, "europe-west1", "dev"), "gke__europe-west1_dev");
    }

    #[test]
    fn parses_a_cluster_list() {
        let json = br#"[
          {
            "name": "prod",
            "location": "us-central1",
            "status": "RUNNING",
            "currentNodeCount": 6,
            "endpoint": "34.66.1.2"
          },
          {
            "name": "dev",
            "location": "europe-west1-b",
            "status": "PROVISIONING"
          }
        ]"#;
        let clusters = parse_clusters(json).unwrap();
        assert_eq!(clusters.len(), 2);
        assert_eq!(
            clusters[0],
            GkeCluster {
                name: "prod".into(),
                location: "us-central1".into(),
                status: "RUNNING".into(),
                node_count: 6,
            }
        );
        // Missing currentNodeCount defaults to 0.
        assert_eq!(clusters[1].node_count, 0);
        assert_eq!(clusters[1].status, "PROVISIONING");
    }

    #[test]
    fn empty_cluster_list_is_ok() {
        assert!(parse_clusters(b"[]").unwrap().is_empty());
    }
}
