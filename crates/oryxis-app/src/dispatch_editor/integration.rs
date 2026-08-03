//! What else may reach into this host: MCP, the AI agent, the monitor,
//! agent and X11 forwarding, session logging, environment variables.

use super::*;

impl Oryxis {
    pub(super) fn handle_editor_integration(&mut self, message: EditorMessage) -> Task<Message> {
        match message {
            EditorMessage::EditorToggleMcpEnabled => {
                self.editor_form.mcp_enabled = !self.editor_form.mcp_enabled;
            }
            EditorMessage::EditorToggleMonitorEnabled => {
                self.editor_form.monitor_enabled = !self.editor_form.monitor_enabled;
            }
            EditorMessage::EditorToggleAgentForwarding => {
                self.editor_form.agent_forwarding = !self.editor_form.agent_forwarding;
            }
            EditorMessage::EditorToggleX11Forwarding => {
                self.editor_form.x11_forwarding = !self.editor_form.x11_forwarding;
            }
            EditorMessage::EditorCycleSessionLogging => {
                self.editor_form.session_logging = match self.editor_form.session_logging {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
            }
            EditorMessage::EditorAddEnvVar => {
                self.editor_form.env_vars.push(EnvVarForm::default());
            }
            EditorMessage::EditorRemoveEnvVar(i) => {
                if i < self.editor_form.env_vars.len() {
                    self.editor_form.env_vars.remove(i);
                }
            }
            EditorMessage::EditorEnvVarKeyChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.key = v;
                }
            }
            EditorMessage::EditorEnvVarValueChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.value = v;
                }
            }
            EditorMessage::EditorCloudTransportChanged(t) => {
                self.editor_form.cloud_transport = Some(t);
            }
            // Routed here by the parent; anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
