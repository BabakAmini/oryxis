//! What the user picks inside an open panel: which workloads to
//! import, the text filter, collapsed sections, and the defaults
//! (transport + target group) the import will apply.
//!
//! All of it is panel-local state, discarded when the panel closes.

use super::*;

impl Oryxis {
    pub(super) fn handle_discover_selection(
        &mut self,
        message: CloudMessage,
    ) -> Result<Task<Message>, CloudMessage> {
        match message {
            CloudMessage::CloudDiscoverToggleEc2(instance_id) => {
                if !self.cloud_discover.selected_ec2.remove(&instance_id) {
                    self.cloud_discover.selected_ec2.insert(instance_id);
                }
            }
            CloudMessage::CloudDiscoverToggleEcs(key) => {
                if !self.cloud_discover.selected_ecs.remove(&key) {
                    self.cloud_discover.selected_ecs.insert(key);
                }
            }
            CloudMessage::CloudDiscoverToggleK8s(key) => {
                if !self.cloud_discover.selected_k8s.remove(&key) {
                    self.cloud_discover.selected_k8s.insert(key);
                }
            }
            CloudMessage::CloudDiscoverFilterChanged(s) => {
                self.cloud_discover.filter = s;
            }
            CloudMessage::CloudDiscoverToggleSection(key) => {
                if !self.cloud_discover.collapsed.remove(&key) {
                    self.cloud_discover.collapsed.insert(key);
                }
            }
            CloudMessage::CloudDiscoverDefaultTransportChanged(t) => {
                self.cloud_discover.default_transport = t;
            }
            CloudMessage::CloudDiscoverDefaultGroupNameChanged(v) => {
                self.cloud_discover.default_group_name = v;
            }
            CloudMessage::CloudDiscoverDefaultGroupPick(label) => {
                self.cloud_discover.default_group_name = label;
                self.cloud_discover.default_group_picker_open = false;
                // The modal-stack injection at `view_main` only
                // checks `self.overlay`; without also clearing it
                // here the menu would re-render on top after every
                // pick. Mirrors the close-branch of
                // `ToggleCloudDiscoverGroupPicker`.
                if matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(crate::state::OverlayContent::CloudDiscoverGroupPicker)
                ) {
                    self.overlay = None;
                }
            }
            CloudMessage::ToggleCloudDiscoverGroupPicker => {
                self.cloud_discover.default_group_picker_open =
                    !self.cloud_discover.default_group_picker_open;
                if self.cloud_discover.default_group_picker_open {
                    self.cloud_discover.default_group_picker_search.clear();
                    // Anchor the menu off the live combo bounds
                    // captured by the `bounds_reporter` wrapping the
                    // Import-into row. `BoundsCell` value is the
                    // last-rendered screen-space rect of the combo
                    // (input + chevron). Menu's top sits 6 px below
                    // the combo's bottom; left edge matches combo
                    // left so the dropdown visually replaces the
                    // input column.
                    let combo = self.cloud_discover.default_group_combo_bounds.get();
                    let gap = 6.0_f32;
                    let x = combo.x.max(0.0);
                    let y = (combo.y + combo.height + gap).max(0.0);
                    self.overlay = Some(crate::state::OverlayState {
                        content: crate::state::OverlayContent::CloudDiscoverGroupPicker,
                        x,
                        y,
                    });
                } else if matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(crate::state::OverlayContent::CloudDiscoverGroupPicker)
                ) {
                    self.overlay = None;
                }
            }
            CloudMessage::CloudDiscoverDefaultGroupPickerSearchChanged(v) => {
                self.cloud_discover.default_group_picker_search = v;
            }
            // The parent routed us here, so a message that is not
            // in this family is a grouping mistake. Hand it back
            // rather than swallow it.
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
