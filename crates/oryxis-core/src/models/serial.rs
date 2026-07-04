//! Per-host serial-line parameters, carried by a `Connection` whose
//! `protocol` is `Serial`. The port path itself reuses
//! `Connection.hostname` (e.g. `COM3` on Windows, `/dev/ttyUSB0` on
//! Unix); everything else lives here.
//!
//! Kept as plain serde enums (not the `serialport`/`tokio-serial`
//! types) so `oryxis-core` stays free of the transport crate; the
//! `oryxis-serial` engine maps these onto the driver enums at open
//! time. `Default` is the universal 9600 8N1, no flow control, no
//! local echo, matching PuTTY / minicom / screen so a user coming
//! from those tools sees the same starting point.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialParams {
    /// Symbol rate (bits per second). Free integer so exotic rates
    /// (e.g. 250000 for some 3D printers) are expressible, not just a
    /// fixed menu.
    pub baud: u32,
    /// Data bits per frame (5..=8). Held as a raw `u8`; the engine
    /// rejects out-of-range values back to 8 at open time.
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
    pub flow_control: SerialFlowControl,
    /// Echo typed characters locally. Raw serial has no ECHO
    /// negotiation (unlike SSH/Telnet), so a device that doesn't echo
    /// leaves the screen blank while typing until this is on.
    #[serde(default)]
    pub local_echo: bool,
    /// What the Enter key sends on the wire. Devices disagree (CR is
    /// the common console default; some want LF or CR LF).
    #[serde(default)]
    pub line_ending: SerialLineEnding,
}

impl Default for SerialParams {
    fn default() -> Self {
        SerialParams {
            baud: 9600,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
            flow_control: SerialFlowControl::None,
            local_echo: false,
            line_ending: SerialLineEnding::Cr,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

impl std::fmt::Display for SerialParity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SerialParity::None => "None",
            SerialParity::Odd => "Odd",
            SerialParity::Even => "Even",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SerialStopBits {
    #[default]
    One,
    Two,
}

impl std::fmt::Display for SerialStopBits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SerialStopBits::One => "1",
            SerialStopBits::Two => "2",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SerialFlowControl {
    #[default]
    None,
    /// XON/XOFF (software).
    Software,
    /// RTS/CTS (hardware).
    Hardware,
}

impl std::fmt::Display for SerialFlowControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SerialFlowControl::None => "None",
            SerialFlowControl::Software => "XON/XOFF",
            SerialFlowControl::Hardware => "RTS/CTS",
        })
    }
}

/// Byte(s) the Enter key emits on a serial line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SerialLineEnding {
    /// Carriage return (`\r`), the common console default.
    #[default]
    Cr,
    /// Line feed (`\n`).
    Lf,
    /// Both (`\r\n`).
    CrLf,
}

impl SerialLineEnding {
    /// The bytes this ending sends for one Enter press.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            SerialLineEnding::Cr => b"\r",
            SerialLineEnding::Lf => b"\n",
            SerialLineEnding::CrLf => b"\r\n",
        }
    }
}

impl std::fmt::Display for SerialLineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SerialLineEnding::Cr => "CR",
            SerialLineEnding::Lf => "LF",
            SerialLineEnding::CrLf => "CR LF",
        })
    }
}

/// Common baud rates offered in the editor picker (also accepts a
/// free-typed value). Ordered low to high.
pub const COMMON_BAUD_RATES: &[u32] = &[
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600,
];
