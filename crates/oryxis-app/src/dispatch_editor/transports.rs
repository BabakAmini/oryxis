//! The non-SSH transports' own fields.
//!
//! Serial (baud, bits, parity, flow, line ending, local echo) and remote
//! desktop (protocol kind, gateway). Both appear only when the protocol
//! picker selects them, which is why they are not in `identity`.

use super::*;

impl Oryxis {
    pub(super) fn handle_editor_transports(&mut self, message: EditorMessage) -> Task<Message> {
        match message {
            EditorMessage::EditorSerialBaudChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).baud = v;
            }
            EditorMessage::EditorSerialDataBitsChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).data_bits = v;
            }
            EditorMessage::EditorSerialParityChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).parity = v;
            }
            EditorMessage::EditorSerialStopBitsChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).stop_bits = v;
            }
            EditorMessage::EditorSerialFlowChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).flow_control = v;
            }
            EditorMessage::EditorSerialLineEndingChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).line_ending = v;
            }
            EditorMessage::EditorSerialLocalEchoToggled => {
                let s = self.editor_form.serial.get_or_insert_with(Default::default);
                s.local_echo = !s.local_echo;
            }
            EditorMessage::EditorRdKindChanged(kind) => {
                // Retarget the port field when it still holds the other
                // kind's default, so a typed port survives the RDP<->VNC
                // switch (the endpoint port reuses the normal port field).
                let old_default = self.editor_form.rd_kind.default_port().to_string();
                if self.editor_form.port.trim() == old_default {
                    self.editor_form.port = kind.default_port().to_string();
                }
                self.editor_form.rd_kind = kind;
            }
            EditorMessage::EditorRdGatewayChanged(id) => {
                self.editor_form.rd_gateway_id = id;
            }
            // Routed here by the parent; anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
