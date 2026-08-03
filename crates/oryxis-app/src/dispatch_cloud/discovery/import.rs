//! Turning the current selection into vault hosts: the confirm
//! prompt, its cancel, and the batch that actually writes.
//!
//! The confirm is never skipped, because it is where the target
//! group and the transport get decided for the whole batch.

use super::*;

impl Oryxis {
    pub(super) fn handle_discover_import(
        &mut self,
        message: CloudMessage,
    ) -> Result<Task<Message>, CloudMessage> {
        match message {
            CloudMessage::CloudDiscoverImport => {
                // Always route through the confirmation modal so the
                // user gets a chance to set the target group (and the
                // transport, when EC2 hosts are part of the batch).
                // Empty selection short-circuits.
                if self.cloud_discover.selected_ec2.is_empty()
                    && self.cloud_discover.selected_ecs.is_empty()
                {
                    return Ok(Task::none());
                }
                self.cloud_import_confirm_visible = true;
            }
            CloudMessage::CloudDiscoverImportCancelled => {
                self.cloud_import_confirm_visible = false;
                self.cloud_discover.default_group_picker_open = false;
            }
            CloudMessage::CloudDiscoverImportConfirmed => {
                self.cloud_import_confirm_visible = false;
                let Some(profile_id) = self.cloud_discover.profile_id else {
                    return Ok(Task::none());
                };
                if !self.cloud_profiles.iter().any(|p| p.id == profile_id) {
                    return Ok(Task::none());
                }
                let CloudDiscoverState::Loaded(result) = &self.cloud_discover.state else {
                    return Ok(Task::none());
                };
                let selected_ec2: Vec<_> = result
                    .ec2
                    .iter()
                    .filter(|e| self.cloud_discover.selected_ec2.contains(&e.instance_id))
                    .cloned()
                    .collect();
                let selected_ecs: Vec<_> = result
                    .ecs_services
                    .iter()
                    .filter(|s| {
                        self.cloud_discover.selected_ecs
                            .contains(&format!("{}/{}/{}", s.cluster, s.service, s.container))
                    })
                    .cloned()
                    .collect();
                let selected_k8s: Vec<_> = result
                    .k8s_workloads
                    .iter()
                    .filter(|w| {
                        self.cloud_discover.selected_k8s
                            .contains(&format!("{}/{}/{}", w.namespace, w.kind, w.name))
                    })
                    .cloned()
                    .collect();
                if selected_ec2.is_empty() && selected_ecs.is_empty() && selected_k8s.is_empty() {
                    return Ok(Task::none());
                }

                if let Some(vault) = &self.vault {
                    // Resolve the target group from the typed name.
                    // Empty = root (no parent). Matching label = reuse
                    // existing group. Non-matching = create a new
                    // group with that label on the spot, so the user
                    // can type any folder name (existing or new) and
                    // have it materialised in one go.
                    let typed = self.cloud_discover.default_group_name.trim().to_string();
                    let provider_id_str = self
                        .cloud_profiles
                        .iter()
                        .find(|p| p.id == profile_id)
                        .map(|p| p.provider.clone())
                        .unwrap_or_default();
                    let profile_label = self
                        .cloud_profiles
                        .iter()
                        .find(|p| p.id == profile_id)
                        .map(|p| p.label.clone())
                        .unwrap_or_default();
                    let provider_group_id: Option<uuid::Uuid> = if typed.is_empty() {
                        None
                    } else {
                        // Breadcrumb-path match first (the picker fills
                        // paths, so a subgroup is a valid import
                        // target), bare label as the typed fallback. An
                        // unmatched value is materialised as a nested
                        // PATH ("Prod / NewTeam" builds the chain,
                        // reusing existing segments) so a typed folder
                        // name with the separator can't mint a single
                        // group that impersonates a real path.
                        let mut created = Vec::new();
                        let gid =
                            Group::resolve_or_create_path(&mut self.groups, &typed, &mut created);
                        // Brand glyph only when the user kept the
                        // profile-label default (always a single
                        // segment). A custom folder name / path gets a
                        // generic icon so it doesn't look like an
                        // auto-folder by accident.
                        if typed == profile_label
                            && let Some(g) = created.iter_mut().find(|g| Some(g.id) == gid)
                        {
                            g.icon = Some(provider_id_str.clone());
                        }
                        for g in &created {
                            let _ = vault.save_group(g);
                        }
                        gid
                    };

                    for e in &selected_ec2 {
                        // Connection labels prefer the EC2 Name tag
                        // when set, otherwise fall back to the
                        // instance id (always unique inside a region).
                        let label = e.name.clone().unwrap_or_else(|| e.instance_id.clone());
                        let hostname = e
                            .public_dns
                            .clone()
                            .or_else(|| e.public_ip.clone())
                            .or_else(|| e.private_dns.clone())
                            .or_else(|| e.private_ip.clone())
                            .unwrap_or_default();

                        let mut conn = Connection::new(label, hostname);
                        // Fall back to `ec2-user` when the discovery
                        // result didn't infer a username, it's the
                        // default on Amazon Linux 2 / 2023 (the most
                        // common AMI family) and Instance Connect
                        // assumes it. Bitnami / Ubuntu users will
                        // need to edit, but that's a smaller hassle
                        // than landing with an empty field.
                        conn.username = e
                            .default_username
                            .clone()
                            .or_else(|| Some("ec2-user".to_string()));
                        conn.group_id = provider_group_id;
                        conn.cloud_ref = Some(CloudRef {
                            profile_id,
                            resource_type: CloudResourceType::Ec2,
                            resource_id: e.instance_id.clone(),
                            region: Some(e.region.clone()),
                            // Honour the per-discovery default the
                            // user picked at the bottom of the panel.
                            // Saves "import → edit each → set
                            // Instance Connect" on bulk imports.
                            transport_pref: self.cloud_discover.default_transport,
                            // Public IPs change across stop/start, so
                            // re-resolving on each connect is the safer
                            // default for imported EC2.
                            auto_refresh_hostname: true,
                            orphaned_at: None,
                        });
                        let _ = vault.save_connection(&conn, None);
                    }

                    // Each picked ECS service becomes a *dynamic
                    // group*: a `Group` row with `cloud_query` set,
                    // parented under the profile group. When the user
                    // expands it later, the resolver lists current
                    // tasks. The actual `EcsExec` transport doesn't
                    // ship until PR 5, clicking a task today gives a
                    // friendly "not implemented" error, but the
                    // structure is already persisted and syncable.
                    for s in &selected_ecs {
                        let label = format!("{} / {}", s.service, s.container);
                        let mut g = Group::new(label);
                        g.parent_id = provider_group_id;
                        // ECS-specific brand glyph (the orange hex box)
                        //, distinguishes it from the AWS-provider
                        // folder one level up at a glance.
                        g.icon = Some("ecs".into());
                        g.cloud_query = Some(CloudQuery {
                            profile_id,
                            kind: CloudQueryKind::EcsTasks {
                                cluster: s.cluster.clone(),
                                service: s.service.clone(),
                                container: s.container.clone(),
                            },
                            template: ConnectionTemplate {
                                username: None,
                                initial_command: None,
                                transport: TransportKind::EcsExec,
                                key_id: None,
                                identity_id: None,
                                terminal_theme: None,
                            },
                        });
                        let _ = vault.save_group(&g);
                    }

                    // Each picked K8s workload becomes a dynamic group
                    // backed by a `K8sPods` label query. Expanding it
                    // resolves the workload's current pods; clicking a pod
                    // opens `kubectl exec`.
                    for w in &selected_k8s {
                        let label = format!("{} ({})", w.name, w.namespace);
                        let mut g = Group::new(label);
                        g.parent_id = provider_group_id;
                        g.icon = Some("kubernetes".into());
                        let selector = oryxis_core::models::cloud::PodSelector::Labels(
                            w.match_labels.clone(),
                        );
                        g.cloud_query = Some(CloudQuery {
                            profile_id,
                            kind: CloudQueryKind::K8sPods {
                                context: w.context.clone(),
                                namespace: w.namespace.clone(),
                                selector,
                            },
                            template: ConnectionTemplate {
                                username: None,
                                initial_command: None,
                                transport: TransportKind::KubectlExec,
                                key_id: None,
                                identity_id: None,
                                terminal_theme: None,
                            },
                        });
                        let _ = vault.save_group(&g);
                    }

                    self.cloud_discover.visible = false;
                    self.cloud_discover.profile_id = None;
                    self.cloud_discover.selected_ec2.clear();
                    self.cloud_discover.selected_ecs.clear();
                    self.cloud_discover.selected_k8s.clear();
                    self.cloud_discover.state = CloudDiscoverState::Idle;
                    self.load_data_from_vault();
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
