//! Microsoft Azure provider for Oryxis, driven entirely through the `az`
//! CLI (no native Azure SDK).
//!
//! Discovery shells out to `az vm list --show-details --output json` and
//! parses the JSON into the same `DiscoveredEc2` family the AWS provider
//! uses for individual-VM import; each virtual machine becomes an
//! importable `Connection` reached over plain SSH. The provider honours
//! the profile's optional `subscription`, mapping every failure into a
//! `CloudError`.
//!
//! `az` must be on PATH and already authenticated (`az login`); a missing
//! binary surfaces as `CloudError::InvalidConfig` so the UI can tell the
//! user to install / sign in. AKS (managed Kubernetes) is handled
//! separately by fetching a kubeconfig via `az aks get-credentials` and
//! delegating to the Kubernetes provider, so it is not a distinct
//! transport in this crate.

mod discover;
pub mod aks;

use async_trait::async_trait;
use serde::Deserialize;

use oryxis_cloud::{
    CloudError, CloudProfile, CloudProvider, CloudQuery, CloudResourceType, DiscoveredHost,
    DiscoveryResult, TransportKind,
};

/// Parsed `CloudProfile.config` for an Azure account.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AzureConfig {
    /// Azure subscription id (or name) to scope every call to. `None`/empty
    /// = whatever `az account show` resolves (the active subscription).
    #[serde(default)]
    pub subscription: Option<String>,
}

impl AzureConfig {
    /// Parse the profile's JSON `config`. A blank / malformed config is
    /// treated as "all defaults" (active subscription) rather than an
    /// error, so a half-filled profile still talks to the default
    /// subscription.
    pub fn from_profile(profile: &CloudProfile) -> Self {
        if profile.config.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str(&profile.config).unwrap_or_default()
    }
}

/// Build the `az` argument list: the subcommand args followed by the
/// global `--subscription` flag from the config (az accepts
/// `--subscription` on every command). Pure + tested.
pub(crate) fn az_args(cfg: &AzureConfig, sub: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = sub.iter().map(|s| s.to_string()).collect();
    if let Some(s) = cfg
        .subscription
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        args.push("--subscription".to_string());
        args.push(s.to_string());
    }
    args
}

/// Map a failed `az` invocation's stderr into the closest `CloudError`
/// variant so the UI can colour / phrase it sensibly.
pub(crate) fn classify_az_error(stderr: &str) -> CloudError {
    let s = stderr.to_lowercase();
    if s.contains("az login")
        || s.contains("please run 'az login'")
        || s.contains("not logged in")
        || s.contains("no subscription found")
        || s.contains("aadsts")
        || s.contains("refresh token")
        || s.contains("token has expired")
        || s.contains("authentication failed")
        || s.contains("credential")
    {
        CloudError::Auth(stderr.trim().to_string())
    } else if s.contains("connection")
        || s.contains("timed out")
        || s.contains("could not be resolved")
        || s.contains("failed to establish a new connection")
        || s.contains("getaddrinfo")
        || s.contains("network is unreachable")
    {
        CloudError::Network(stderr.trim().to_string())
    } else if s.contains("missingsubscriptionregistration")
        || s.contains("not registered to use namespace")
        || s.contains("subscriptionnotfound")
        || s.contains("subscription") && s.contains("not found")
        || s.contains("was not found")
    {
        // A subscription / resource-provider registration problem on the
        // Azure side, actionable config. Matched by the specific Azure
        // error phrasing, not a bare substring.
        CloudError::InvalidConfig(stderr.trim().to_string())
    } else {
        CloudError::Upstream(stderr.trim().to_string())
    }
}

/// Azure CLI executable candidates, tried in order until one spawns. On
/// Windows the MSI-installed CLI ships as `az.cmd` (a batch wrapper with
/// no `az.exe`), which `Command::new("az")` would fail to resolve,
/// `CreateProcess` only appends `.exe`, not `.cmd`, so the wrapper is
/// named explicitly first; Rust (>= 1.77) applies safe batch-argument
/// escaping when the target ends in `.cmd`. A pip-installed CLI ships an
/// `az.exe` console script instead (and no `az.cmd`), so a NotFound on
/// `az.cmd` falls back to plain `az`, which resolves the `.exe`.
#[cfg(windows)]
const AZ_BINS: &[&str] = &["az.cmd", "az"];
#[cfg(not(windows))]
const AZ_BINS: &[&str] = &["az"];

/// Run `az <sub...> --subscription <s> --only-show-errors` and return
/// stdout bytes on success. `--only-show-errors` keeps warnings out of the
/// stderr we classify on failure, and gives a TTY-less subprocess clean,
/// machine-parseable diagnostics. It is appended *after* the subcommand
/// (like `--subscription`): the Azure CLI takes its global flags in the
/// trailing position, not the leading one gcloud accepts. Every call this
/// crate makes is read-only or a kubeconfig write, so it is side-effect
/// safe.
pub(crate) async fn run_az(cfg: &AzureConfig, sub: &[&str]) -> Result<Vec<u8>, CloudError> {
    let mut args = az_args(cfg, sub);
    args.push("--only-show-errors".to_string());
    let mut output = None;
    for bin in AZ_BINS {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(&args);
        // On Windows the `.cmd` wrapper would flash a console window over
        // the GUI on every call. 0x08000000 = CREATE_NO_WINDOW suppresses
        // it (same guard the app uses for its own wsl.exe spawns).
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        match cmd.output().await {
            Ok(o) => {
                output = Some(o);
                break;
            }
            // Not on PATH under this name: try the next candidate.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(CloudError::Other(format!("failed to run az: {e}"))),
        }
    }
    let Some(output) = output else {
        // Every candidate was NotFound: the CLI is genuinely missing.
        return Err(CloudError::InvalidConfig(
            "az was not found on PATH. Install the Azure CLI and run \
             `az login` to use Azure."
                .into(),
        ));
    };
    if !output.status.success() {
        return Err(classify_az_error(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

/// Azure provider. Stateless, every call re-derives config from the
/// profile and shells out to `az`.
#[derive(Debug, Default, Clone, Copy)]
pub struct AzureProvider;

impl AzureProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CloudProvider for AzureProvider {
    fn id(&self) -> &'static str {
        "azure"
    }

    async fn test_credentials(&self, profile: &CloudProfile) -> Result<(), CloudError> {
        let cfg = AzureConfig::from_profile(profile);
        // Validate the identity / subscription, not any one resource API.
        // This provider serves both VMs and AKS, so a VM-specific probe
        // (`vm list`) would wrongly fail a caller who has AKS access but
        // lacks VM read on the subscription. `az account show` exercises
        // the exact login `az` uses for discovery and fails cleanly
        // (classified `Auth`) when the user is not logged in / needs
        // reauth, or (classified `InvalidConfig`) when the configured
        // subscription is not accessible. Per-API validity surfaces later,
        // at discovery.
        run_az(&cfg, &["account", "show", "--output", "json"]).await?;
        Ok(())
    }

    async fn discover(&self, profile: &CloudProfile) -> Result<DiscoveryResult, CloudError> {
        let cfg = AzureConfig::from_profile(profile);
        // VMs and AKS are independent resource providers on a
        // subscription: one can be permitted without the other. Probe
        // both, and let each half contribute what it can. `discover_aks`
        // is already best-effort (empty on any listing failure); mirror
        // that for VMs so a VM-only failure (no permission) does not hide
        // the AKS clusters the user CAN see.
        let vm_result = discover::discover_vms(&cfg).await;
        let aks_clusters = aks::discover_clusters(&cfg).await.unwrap_or_default();
        let ec2 = match vm_result {
            Ok(v) => v,
            // VM listing failed. If AKS also produced nothing, the failure
            // is the real root cause (bad auth / subscription) and must
            // surface; if AKS returned clusters, the subscription simply
            // lacks VM read, so show what we have instead of failing the
            // whole discovery.
            Err(e) if aks_clusters.is_empty() => return Err(e),
            Err(_) => Vec::new(),
        };
        Ok(DiscoveryResult {
            ec2,
            ecs_services: Vec::new(),
            k8s_workloads: Vec::new(),
            gke_clusters: Vec::new(),
            aks_clusters,
        })
    }

    async fn resolve_query(
        &self,
        _profile: &CloudProfile,
        _query: &CloudQuery,
    ) -> Result<Vec<DiscoveredHost>, CloudError> {
        // Azure VMs have no dynamic-group family (the ECS / K8s-workload
        // analog); every VM imports as a standalone Connection instead.
        // AKS clusters are served through the Kubernetes provider.
        Err(CloudError::Unsupported("azure resolve_query".into()))
    }

    fn supported_transports(&self, resource_type: CloudResourceType) -> Vec<TransportKind> {
        match resource_type {
            // Azure VMs are reached over plain SSH (public or internal IP).
            // Bastion / AAD SSH login are future work.
            CloudResourceType::Ec2 => vec![TransportKind::Ssh],
        }
    }

    async fn aks_get_credentials(
        &self,
        profile: &CloudProfile,
        cluster: &str,
        resource_group: &str,
    ) -> Result<String, CloudError> {
        let cfg = AzureConfig::from_profile(profile);
        aks::get_credentials(&cfg, cluster, resource_group).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(config: &str) -> CloudProfile {
        let mut p = CloudProfile::new("azure", "azure");
        p.auth_kind = "az".into();
        p.config = config.into();
        p
    }

    #[test]
    fn config_parses_subscription_or_defaults() {
        let with = AzureConfig::from_profile(&profile_with(r#"{"subscription":"sub-123"}"#));
        assert_eq!(with.subscription.as_deref(), Some("sub-123"));
        // Blank / malformed config yields all-defaults (active subscription).
        assert!(AzureConfig::from_profile(&profile_with(""))
            .subscription
            .is_none());
        assert!(AzureConfig::from_profile(&profile_with("not json"))
            .subscription
            .is_none());
    }

    #[test]
    fn az_args_appends_subscription_only_when_set() {
        let sub = &["vm", "list", "--show-details", "--output", "json"];
        let none = az_args(&AzureConfig::default(), sub);
        assert_eq!(none, sub.to_vec());

        let cfg = AzureConfig {
            subscription: Some("sub-123".into()),
        };
        let with = az_args(&cfg, sub);
        assert_eq!(
            with,
            vec![
                "vm",
                "list",
                "--show-details",
                "--output",
                "json",
                "--subscription",
                "sub-123"
            ]
        );

        // A whitespace-only subscription is treated as unset.
        let blank = AzureConfig {
            subscription: Some("  ".into()),
        };
        assert_eq!(az_args(&blank, sub), sub.to_vec());
    }

    #[test]
    fn error_classification_buckets_stderr() {
        assert!(matches!(
            classify_az_error("ERROR: Please run 'az login' to setup account."),
            CloudError::Auth(_)
        ));
        assert!(matches!(
            classify_az_error("ERROR: AADSTS700082: The refresh token has expired"),
            CloudError::Auth(_)
        ));
        assert!(matches!(
            classify_az_error(
                "ERROR: (MissingSubscriptionRegistration) The subscription is not registered to use namespace 'Microsoft.ContainerService'"
            ),
            CloudError::InvalidConfig(_)
        ));
        assert!(matches!(
            classify_az_error("ERROR: Could not be resolved: connection timed out"),
            CloudError::Network(_)
        ));
        assert!(matches!(
            classify_az_error("ERROR: something else entirely"),
            CloudError::Upstream(_)
        ));
    }
}
