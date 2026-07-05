//! Compute Engine instance discovery: `gcloud compute instances list
//! --format=json`, parsed into the `DiscoveredEc2` family shared with the
//! AWS provider (individual-VM import).

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredEc2};

use crate::{run_gcloud, GcpConfig};

/// One instance as emitted by `gcloud compute instances list
/// --format=json`. gcloud uses camelCase keys; only the fields we map
/// are declared, the rest are ignored.
#[derive(Debug, Deserialize)]
struct GceInstance {
    name: String,
    /// Full zone URL, e.g. `.../zones/us-central1-a`. The short zone is
    /// the last path segment.
    #[serde(default)]
    zone: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "networkInterfaces", default)]
    network_interfaces: Vec<GceNic>,
}

#[derive(Debug, Deserialize)]
struct GceNic {
    /// Internal (private) IP of the interface.
    #[serde(rename = "networkIP", default)]
    network_ip: Option<String>,
    /// External access configs; the first `natIP` is the public IP.
    #[serde(rename = "accessConfigs", default)]
    access_configs: Vec<GceAccessConfig>,
}

#[derive(Debug, Deserialize)]
struct GceAccessConfig {
    #[serde(rename = "natIP", default)]
    nat_ip: Option<String>,
}

/// Short zone name from a full zone URL (`.../zones/us-central1-a` ->
/// `us-central1-a`). Falls back to the input when it isn't a URL.
fn short_zone(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).to_string()
}

/// A blank string is treated as absent.
fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Parse `gcloud compute instances list --format=json` output into the
/// shared `DiscoveredEc2` shape. Pure, so it is unit-tested against
/// fixture JSON without a live `gcloud`.
fn parse_instances(json: &[u8]) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let instances: Vec<GceInstance> = serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing gcloud instances JSON: {e}")))?;
    Ok(instances.into_iter().map(map_instance).collect())
}

fn map_instance(inst: GceInstance) -> DiscoveredEc2 {
    let nic = inst.network_interfaces.into_iter().next();
    let (private_ip, public_ip) = match nic {
        Some(n) => {
            let public = n
                .access_configs
                .into_iter()
                .find_map(|ac| non_empty(ac.nat_ip));
            (non_empty(n.network_ip), public)
        }
        None => (None, None),
    };
    DiscoveredEc2 {
        // GCP identifies an instance by name for connect purposes (it is
        // what `gcloud compute ssh` takes); the numeric id is not used.
        instance_id: inst.name.clone(),
        // Store the short zone in the region slot (free-text location the
        // UI shows and groups by; GCP is zonal, so the zone is the useful
        // granularity).
        region: short_zone(&inst.zone),
        name: Some(inst.name),
        // GCP exposes no public/private DNS names the way EC2 does.
        public_dns: None,
        private_dns: None,
        public_ip,
        private_ip,
        // Lowercased to match the AWS provider's convention ("running").
        state: inst.status.to_lowercase(),
        // The SSH user depends on OS Login / project metadata; the editor
        // lets the user set it (no reliable inference from the API).
        default_username: None,
    }
}

/// Discover every Compute Engine instance visible to the profile.
pub(crate) async fn discover_instances(
    cfg: &GcpConfig,
) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let out = run_gcloud(
        cfg,
        &["compute", "instances", "list", "--format=json"],
    )
    .await?;
    parse_instances(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_zone_extracts_last_segment() {
        assert_eq!(
            short_zone("https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a"),
            "us-central1-a"
        );
        assert_eq!(short_zone("europe-west1-b"), "europe-west1-b");
    }

    #[test]
    fn parses_a_running_instance_with_public_and_private_ips() {
        let json = br#"[
          {
            "name": "web-1",
            "zone": "https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a",
            "status": "RUNNING",
            "networkInterfaces": [
              {
                "networkIP": "10.128.0.2",
                "accessConfigs": [ { "natIP": "34.66.1.2", "type": "ONE_TO_ONE_NAT" } ]
              }
            ]
          }
        ]"#;
        let hosts = parse_instances(json).unwrap();
        assert_eq!(hosts.len(), 1);
        let h = &hosts[0];
        assert_eq!(h.instance_id, "web-1");
        assert_eq!(h.name.as_deref(), Some("web-1"));
        assert_eq!(h.region, "us-central1-a");
        assert_eq!(h.state, "running");
        assert_eq!(h.public_ip.as_deref(), Some("34.66.1.2"));
        assert_eq!(h.private_ip.as_deref(), Some("10.128.0.2"));
        assert!(h.public_dns.is_none() && h.private_dns.is_none());
    }

    #[test]
    fn parses_a_private_only_instance() {
        // No accessConfigs -> no public IP; still importable via internal IP.
        let json = br#"[
          {
            "name": "db-1",
            "zone": ".../zones/europe-west1-b",
            "status": "TERMINATED",
            "networkInterfaces": [ { "networkIP": "10.0.0.5", "accessConfigs": [] } ]
          }
        ]"#;
        let hosts = parse_instances(json).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].public_ip, None);
        assert_eq!(hosts[0].private_ip.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].state, "terminated");
    }

    #[test]
    fn empty_list_is_ok() {
        assert!(parse_instances(b"[]").unwrap().is_empty());
    }
}
