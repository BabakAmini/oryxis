//! Virtual Machine discovery: `az vm list --show-details --output json`,
//! parsed into the `DiscoveredEc2` family shared with the AWS provider
//! (individual-VM import). `--show-details` is what makes az fill in the
//! power state and the public / private IPs (a plain `az vm list` omits
//! them).

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredEc2};

use crate::{run_az, AzureConfig};

/// One VM as emitted by `az vm list --show-details --output json`. az uses
/// camelCase keys; only the fields we map are declared, the rest ignored.
#[derive(Debug, Deserialize)]
struct AzVm {
    name: String,
    #[serde(default)]
    location: String,
    /// e.g. `VM running`, `VM stopped`, `VM deallocated`. Only present
    /// with `--show-details`.
    #[serde(rename = "powerState", default)]
    power_state: String,
    /// Comma-separated public IPs (`--show-details`); empty when none.
    #[serde(rename = "publicIps", default)]
    public_ips: String,
    /// Comma-separated private IPs (`--show-details`).
    #[serde(rename = "privateIps", default)]
    private_ips: String,
    /// Comma-separated FQDNs (`--show-details`); empty when none.
    #[serde(default)]
    fqdns: String,
    /// OS profile carries the admin username Azure provisioned. Absent for
    /// VMs created from a specialized image.
    #[serde(rename = "osProfile", default)]
    os_profile: Option<AzOsProfile>,
}

#[derive(Debug, Deserialize)]
struct AzOsProfile {
    #[serde(rename = "adminUsername", default)]
    admin_username: Option<String>,
}

/// First entry of a comma-separated az field, trimmed. `None` when empty.
fn first_csv(s: &str) -> Option<String> {
    s.split(',')
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

/// A blank string is treated as absent.
fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Normalize az's power state (`VM running`) to the AWS provider's
/// lowercase convention (`running`) by dropping the leading `vm ` marker.
fn normalize_power_state(power: &str) -> String {
    let lower = power.trim().to_lowercase();
    lower.strip_prefix("vm ").unwrap_or(&lower).to_string()
}

/// Parse `az vm list --show-details --output json` output into the shared
/// `DiscoveredEc2` shape. Pure, so it is unit-tested against fixture JSON
/// without a live `az`.
fn parse_vms(json: &[u8]) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let vms: Vec<AzVm> = serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing az vm JSON: {e}")))?;
    Ok(vms.into_iter().map(map_vm).collect())
}

fn map_vm(vm: AzVm) -> DiscoveredEc2 {
    let default_username = vm
        .os_profile
        .and_then(|p| p.admin_username)
        .filter(|u| !u.trim().is_empty());
    DiscoveredEc2 {
        // Azure addresses a VM by name (+ resource group) for `az vm`
        // commands; for connect purposes the IP is what matters, so the
        // name is the stable id we surface (mirrors the GCP provider).
        instance_id: vm.name.clone(),
        // Store the Azure region in the region slot (free-text location the
        // UI shows and groups by).
        region: vm.location,
        name: Some(vm.name),
        public_dns: non_empty(first_csv(&vm.fqdns)),
        private_dns: None,
        public_ip: non_empty(first_csv(&vm.public_ips)),
        private_ip: non_empty(first_csv(&vm.private_ips)),
        state: normalize_power_state(&vm.power_state),
        // Azure reports the provisioned admin user directly, unlike GCP.
        default_username,
    }
}

/// Discover every VM visible to the profile's subscription.
pub(crate) async fn discover_vms(cfg: &AzureConfig) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let out = run_az(
        cfg,
        &["vm", "list", "--show-details", "--output", "json"],
    )
    .await?;
    parse_vms(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_power_state_drops_vm_prefix() {
        assert_eq!(normalize_power_state("VM running"), "running");
        assert_eq!(normalize_power_state("VM deallocated"), "deallocated");
        // A bare state (no `VM ` marker) is just lowercased.
        assert_eq!(normalize_power_state("Stopped"), "stopped");
    }

    #[test]
    fn parses_a_running_vm_with_public_and_private_ips() {
        let json = br#"[
          {
            "name": "web-1",
            "location": "eastus",
            "powerState": "VM running",
            "publicIps": "20.1.2.3",
            "privateIps": "10.0.0.4",
            "fqdns": "web-1.eastus.cloudapp.azure.com",
            "osProfile": { "adminUsername": "azureuser" }
          }
        ]"#;
        let hosts = parse_vms(json).unwrap();
        assert_eq!(hosts.len(), 1);
        let h = &hosts[0];
        assert_eq!(h.instance_id, "web-1");
        assert_eq!(h.name.as_deref(), Some("web-1"));
        assert_eq!(h.region, "eastus");
        assert_eq!(h.state, "running");
        assert_eq!(h.public_ip.as_deref(), Some("20.1.2.3"));
        assert_eq!(h.private_ip.as_deref(), Some("10.0.0.4"));
        assert_eq!(
            h.public_dns.as_deref(),
            Some("web-1.eastus.cloudapp.azure.com")
        );
        assert_eq!(h.default_username.as_deref(), Some("azureuser"));
    }

    #[test]
    fn parses_a_private_only_vm() {
        // No public IP / FQDN; still importable via internal IP. Multiple
        // private IPs: the first is taken.
        let json = br#"[
          {
            "name": "db-1",
            "location": "westeurope",
            "powerState": "VM deallocated",
            "publicIps": "",
            "privateIps": "10.0.0.5, 10.0.0.6",
            "fqdns": ""
          }
        ]"#;
        let hosts = parse_vms(json).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].public_ip, None);
        assert_eq!(hosts[0].public_dns, None);
        assert_eq!(hosts[0].private_ip.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].state, "deallocated");
        // No osProfile -> no inferred username.
        assert_eq!(hosts[0].default_username, None);
    }

    #[test]
    fn empty_list_is_ok() {
        assert!(parse_vms(b"[]").unwrap().is_empty());
    }
}
