//! Google Cloud Platform provider for Oryxis, driven entirely through the
//! `gcloud` CLI (no native Google SDK).
//!
//! Discovery shells out to `gcloud compute instances list --format=json`
//! and parses the JSON into the same `DiscoveredEc2` family the AWS
//! provider uses for individual-VM import; each Compute Engine instance
//! becomes an importable `Connection` reached over plain SSH. The
//! provider honours the profile's optional `project`, mapping every
//! failure into a `CloudError`.
//!
//! `gcloud` must be on PATH and already authenticated (`gcloud auth
//! login`); a missing binary surfaces as `CloudError::InvalidConfig` so
//! the UI can tell the user to install / sign in. GKE (managed
//! Kubernetes) is handled separately by fetching a kubeconfig via
//! `gcloud container clusters get-credentials` and delegating to the
//! Kubernetes provider, so it is not part of this crate's surface.

mod discover;
pub mod gke;

use async_trait::async_trait;
use serde::Deserialize;

use oryxis_cloud::{
    CloudError, CloudProfile, CloudProvider, CloudQuery, CloudResourceType, DiscoveredHost,
    DiscoveryResult, TransportKind,
};

/// Parsed `CloudProfile.config` for a GCP account.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GcpConfig {
    /// GCP project id to scope every call to. `None`/empty = whatever
    /// `gcloud config get-value project` resolves (the active project).
    #[serde(default)]
    pub project: Option<String>,
}

impl GcpConfig {
    /// Parse the profile's JSON `config`. A blank / malformed config is
    /// treated as "all defaults" (active project) rather than an error,
    /// so a half-filled profile still talks to the default project.
    pub fn from_profile(profile: &CloudProfile) -> Self {
        if profile.config.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str(&profile.config).unwrap_or_default()
    }
}

/// Build the `gcloud` argument list: the subcommand args followed by the
/// global `--project` flag from the config (gcloud accepts `--project`
/// in any position). Pure + tested.
pub(crate) fn gcloud_args(cfg: &GcpConfig, sub: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = sub.iter().map(|s| s.to_string()).collect();
    if let Some(p) = cfg.project.as_deref().filter(|s| !s.trim().is_empty()) {
        args.push("--project".to_string());
        args.push(p.to_string());
    }
    args
}

/// Map a failed `gcloud` invocation's stderr into the closest
/// `CloudError` variant so the UI can colour / phrase it sensibly.
pub(crate) fn classify_gcloud_error(stderr: &str) -> CloudError {
    let s = stderr.to_lowercase();
    if s.contains("do not have permission")
        || s.contains("permission denied")
        || s.contains("does not have")
        || s.contains("reauthentication")
        || s.contains("credentials")
        || s.contains("not logged in")
        || s.contains("gcloud auth login")
        || s.contains("was not found") && s.contains("project")
    {
        CloudError::Auth(stderr.trim().to_string())
    } else if s.contains("could not reach")
        || s.contains("connection")
        || s.contains("timed out")
        || s.contains("network is unreachable")
        || s.contains("dial tcp")
    {
        CloudError::Network(stderr.trim().to_string())
    } else if s.contains("has not been used")
        || s.contains("api")
        || s.contains("is not enabled")
    {
        // Compute Engine API disabled on the project, actionable config.
        CloudError::InvalidConfig(stderr.trim().to_string())
    } else {
        CloudError::Upstream(stderr.trim().to_string())
    }
}

/// Run `gcloud <sub...> --project <p>` and return stdout bytes on success.
pub(crate) async fn run_gcloud(cfg: &GcpConfig, sub: &[&str]) -> Result<Vec<u8>, CloudError> {
    let args = gcloud_args(cfg, sub);
    let output = tokio::process::Command::new("gcloud")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CloudError::InvalidConfig(
                    "gcloud was not found on PATH. Install the Google Cloud CLI and run \
                     `gcloud auth login` to use GCP."
                        .into(),
                )
            } else {
                CloudError::Other(format!("failed to run gcloud: {e}"))
            }
        })?;
    if !output.status.success() {
        return Err(classify_gcloud_error(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    Ok(output.stdout)
}

/// GCP provider. Stateless, every call re-derives config from the
/// profile and shells out to `gcloud`.
#[derive(Debug, Default, Clone, Copy)]
pub struct GcpProvider;

impl GcpProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CloudProvider for GcpProvider {
    fn id(&self) -> &'static str {
        "gcp"
    }

    async fn test_credentials(&self, profile: &CloudProfile) -> Result<(), CloudError> {
        let cfg = GcpConfig::from_profile(profile);
        // One cheap Compute list exercises exactly what discovery needs:
        // an authenticated identity, a resolvable project, and the
        // Compute Engine API enabled. `--limit=1` keeps it O(1).
        run_gcloud(
            &cfg,
            &["compute", "instances", "list", "--limit=1", "--format=json"],
        )
        .await?;
        Ok(())
    }

    async fn discover(&self, profile: &CloudProfile) -> Result<DiscoveryResult, CloudError> {
        let cfg = GcpConfig::from_profile(profile);
        let ec2 = discover::discover_instances(&cfg).await?;
        Ok(DiscoveryResult {
            ec2,
            ecs_services: Vec::new(),
            k8s_workloads: Vec::new(),
        })
    }

    async fn resolve_query(
        &self,
        _profile: &CloudProfile,
        _query: &CloudQuery,
    ) -> Result<Vec<DiscoveredHost>, CloudError> {
        // GCP Compute has no dynamic-group family (the ECS / K8s-workload
        // analog); every VM imports as a standalone Connection instead.
        // GKE clusters are served through the Kubernetes provider.
        Err(CloudError::Unsupported("gcp resolve_query".into()))
    }

    fn supported_transports(&self, resource_type: CloudResourceType) -> Vec<TransportKind> {
        match resource_type {
            // Compute Engine VMs are reached over plain SSH (public or
            // internal IP). IAP tunnelling / OS Login are future work.
            CloudResourceType::Ec2 => vec![TransportKind::Ssh],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(config: &str) -> CloudProfile {
        let mut p = CloudProfile::new("gcp", "gcp");
        p.auth_kind = "gcloud".into();
        p.config = config.into();
        p
    }

    #[test]
    fn config_parses_project_or_defaults() {
        let with = GcpConfig::from_profile(&profile_with(r#"{"project":"my-proj"}"#));
        assert_eq!(with.project.as_deref(), Some("my-proj"));
        // Blank / malformed config yields all-defaults (active project).
        assert!(GcpConfig::from_profile(&profile_with("")).project.is_none());
        assert!(GcpConfig::from_profile(&profile_with("not json")).project.is_none());
    }

    #[test]
    fn gcloud_args_appends_project_only_when_set() {
        let sub = &["compute", "instances", "list", "--format=json"];
        let none = gcloud_args(&GcpConfig::default(), sub);
        assert_eq!(none, sub.to_vec());

        let cfg = GcpConfig { project: Some("my-proj".into()) };
        let with = gcloud_args(&cfg, sub);
        assert_eq!(
            with,
            vec![
                "compute",
                "instances",
                "list",
                "--format=json",
                "--project",
                "my-proj"
            ]
        );

        // A whitespace-only project is treated as unset.
        let blank = GcpConfig { project: Some("  ".into()) };
        assert_eq!(gcloud_args(&blank, sub), sub.to_vec());
    }

    #[test]
    fn error_classification_buckets_stderr() {
        assert!(matches!(
            classify_gcloud_error("ERROR: (gcloud.compute.instances.list) You do not have permission"),
            CloudError::Auth(_)
        ));
        assert!(matches!(
            classify_gcloud_error("ERROR: gcloud auth login required, not logged in"),
            CloudError::Auth(_)
        ));
        assert!(matches!(
            classify_gcloud_error("ERROR: Compute Engine API has not been used in project x before or it is not enabled"),
            CloudError::InvalidConfig(_)
        ));
        assert!(matches!(
            classify_gcloud_error("ERROR: could not reach the server, connection timed out"),
            CloudError::Network(_)
        ));
        assert!(matches!(
            classify_gcloud_error("ERROR: something else entirely"),
            CloudError::Upstream(_)
        ));
    }
}
