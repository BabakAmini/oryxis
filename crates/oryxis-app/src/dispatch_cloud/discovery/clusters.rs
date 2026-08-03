//! Adding a managed Kubernetes cluster (GKE, AKS) as a dynamic
//! group, straight from the discovery list.
//!
//! Both providers follow the same three steps: ask for the cluster,
//! fetch credentials into the kubeconfig off-thread, then record
//! the result. They are here rather than with `import` because they
//! create a live query, not a batch of hosts.

use super::*;

impl Oryxis {
    pub(super) fn handle_discover_clusters(
        &mut self,
        message: CloudMessage,
    ) -> Result<Task<Message>, CloudMessage> {
        match message {
            CloudMessage::CloudDiscoverAddGke { cluster, location } => {
                // Add a GKE cluster: run get-credentials through the GCP
                // provider (writes the kubeconfig), then create a K8s
                // account pointed at the resulting context. Discovering
                // that account then lists its workloads.
                let Some(profile_id) = self.cloud_discover.profile_id else {
                    return Ok(Task::none());
                };
                let Some(mut profile) = self
                    .cloud_profiles
                    .iter()
                    .find(|p| p.id == profile_id)
                    .cloned()
                else {
                    return Ok(Task::none());
                };
                let registry: Arc<CloudProviderRegistry> =
                    self.cloud_provider_registry.clone();
                let Some(provider) = registry.get(&profile.provider) else {
                    return Ok(Task::none());
                };
                if let Some(vault) = &self.vault {
                    profile.secret =
                        vault.get_cloud_profile_secret(&profile_id).ok().flatten();
                }
                // Label the new K8s account after the cluster so it reads
                // clearly in the accounts list.
                let label = format!("GKE: {cluster}");
                return Ok(Task::perform(
                    async move {
                        let context = provider
                            .gke_get_credentials(&profile, &cluster, &location)
                            .await?;
                        Ok::<(String, String), oryxis_cloud::CloudError>((label, context))
                    },
                    |res| match res {
                        Ok((label, context)) => {
                            Message::Cloud(CloudMessage::CloudDiscoverGkeCredentials(label, context))
                        }
                        Err(e) => Message::Cloud(CloudMessage::CloudDiscoverGkeAdded(Err(e.to_string()))),
                    },
                ));
            }
            CloudMessage::CloudDiscoverGkeCredentials(label, context) => {
                // Credentials fetched: create + save the K8s profile
                // (auth = kubeconfig, default file, the GKE context) unless
                // one already points at this context (idempotent re-add).
                let Some(vault) = self.vault.as_ref() else {
                    return Ok(Task::none());
                };
                let exists = self.cloud_profiles.iter().any(|p| {
                    p.provider == "k8s"
                        && serde_json::from_str::<serde_json::Value>(&p.config)
                            .ok()
                            .and_then(|v| {
                                v.get("context").and_then(|c| c.as_str()).map(str::to_string)
                            })
                            .as_deref()
                            == Some(context.as_str())
                });
                if !exists {
                    let mut profile = oryxis_core::models::CloudProfile::new(label, "k8s");
                    profile.auth_kind = "kubeconfig".to_string();
                    profile.config =
                        serde_json::json!({ "context": context }).to_string();
                    if let Err(e) = vault.save_cloud_profile(&profile, None) {
                        return Ok(self.show_toast(format!(
                            "{}: {e}",
                            crate::i18n::t("cloud_gke_add_failed")
                        )));
                    }
                    self.load_data_from_vault();
                }
                return Ok(self.show_toast(crate::i18n::t("cloud_gke_added").to_string()));
            }
            CloudMessage::CloudDiscoverGkeAdded(result) => {
                if let Err(e) = result {
                    return Ok(self.show_toast(format!(
                        "{}: {e}",
                        crate::i18n::t("cloud_gke_add_failed")
                    )));
                }
            }
            CloudMessage::CloudDiscoverAddAks {
                cluster,
                resource_group,
            } => {
                // Add an AKS cluster: run get-credentials through the Azure
                // provider (writes the kubeconfig), then create a K8s
                // account pointed at the resulting context. Discovering
                // that account then lists its workloads. Mirrors the GKE
                // path; AKS keys credentials by resource group, not region.
                //
                // Dup-guard before the fetch: a k8s account may already
                // point at this cluster, either under the composite
                // `<cluster>-<resource_group>` context this build mints
                // (mirrors `oryxis-cloud-azure::aks::context_name`) or
                // under the bare cluster name older builds stored (az's
                // default context name). Bail with the "added" toast
                // instead of minting a duplicate; the bare form cannot be
                // checked post-fetch (the returned context is composite),
                // so it must be recognized here.
                // Must match `aks::context_name` in the azure plugin (the
                // source of truth; the plugin boundary keeps us from
                // importing it). `.` separator, not `-`: a cluster name
                // can't contain a dot, so `cluster.rg` never collides the
                // way `cluster-rg` did across hyphenated resource groups.
                let composite = format!("{cluster}.{resource_group}");
                let already = self.cloud_profiles.iter().any(|p| {
                    p.provider == "k8s"
                        && serde_json::from_str::<serde_json::Value>(&p.config)
                            .ok()
                            .and_then(|v| {
                                v.get("context").and_then(|c| c.as_str()).map(str::to_string)
                            })
                            .is_some_and(|c| c == composite || c == cluster)
                });
                if already {
                    return Ok(self.show_toast(crate::i18n::t("cloud_aks_added").to_string()));
                }
                let Some(profile_id) = self.cloud_discover.profile_id else {
                    return Ok(Task::none());
                };
                let Some(mut profile) = self
                    .cloud_profiles
                    .iter()
                    .find(|p| p.id == profile_id)
                    .cloned()
                else {
                    return Ok(Task::none());
                };
                let registry: Arc<CloudProviderRegistry> = self.cloud_provider_registry.clone();
                let Some(provider) = registry.get(&profile.provider) else {
                    return Ok(Task::none());
                };
                if let Some(vault) = &self.vault {
                    profile.secret = vault.get_cloud_profile_secret(&profile_id).ok().flatten();
                }
                // Label the new K8s account after the cluster so it reads
                // clearly in the accounts list.
                let label = format!("AKS: {cluster}");
                return Ok(Task::perform(
                    async move {
                        let context = provider
                            .aks_get_credentials(&profile, &cluster, &resource_group)
                            .await?;
                        Ok::<(String, String), oryxis_cloud::CloudError>((label, context))
                    },
                    |res| match res {
                        Ok((label, context)) => Message::Cloud(CloudMessage::CloudDiscoverAksCredentials(label, context)),
                        Err(e) => Message::Cloud(CloudMessage::CloudDiscoverAksAdded(Err(e.to_string()))),
                    },
                ));
            }
            CloudMessage::CloudDiscoverAksCredentials(label, context) => {
                // Credentials fetched: create + save the K8s profile
                // (auth = kubeconfig, default file, the AKS context) unless
                // one already points at this context (idempotent re-add).
                let Some(vault) = self.vault.as_ref() else {
                    return Ok(Task::none());
                };
                let exists = self.cloud_profiles.iter().any(|p| {
                    p.provider == "k8s"
                        && serde_json::from_str::<serde_json::Value>(&p.config)
                            .ok()
                            .and_then(|v| {
                                v.get("context").and_then(|c| c.as_str()).map(str::to_string)
                            })
                            .as_deref()
                            == Some(context.as_str())
                });
                if !exists {
                    let mut profile = oryxis_core::models::CloudProfile::new(label, "k8s");
                    profile.auth_kind = "kubeconfig".to_string();
                    profile.config = serde_json::json!({ "context": context }).to_string();
                    if let Err(e) = vault.save_cloud_profile(&profile, None) {
                        return Ok(self.show_toast(format!(
                            "{}: {e}",
                            crate::i18n::t("cloud_aks_add_failed")
                        )));
                    }
                    self.load_data_from_vault();
                }
                return Ok(self.show_toast(crate::i18n::t("cloud_aks_added").to_string()));
            }
            CloudMessage::CloudDiscoverAksAdded(result) => {
                if let Err(e) = result {
                    return Ok(self.show_toast(format!(
                        "{}: {e}",
                        crate::i18n::t("cloud_aks_add_failed")
                    )));
                }
            }
            // The parent routed us here, so a message that is not
            // in this family is a grouping mistake. Hand it back
            // rather than swallow it.
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
