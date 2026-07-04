//! RFC 854/855 Telnet option negotiation as a pure state machine.
//!
//! `Negotiator::receive` splits an inbound byte stream into application
//! data (IAC sequences stripped, NVT line-ending rules applied) and the
//! protocol replies that must go back on the wire. Option state follows
//! the full RFC 1143 "Q method" on both sides, which is what makes
//! negotiation loops (the classic WILL/DONT ping-pong between two
//! naive implementations) structurally impossible.
//!
//! Options this client plays:
//! - ECHO (1) and SGA (3) are requested *of the server* so the session
//!   runs character-at-a-time with remote echo, like a real terminal.
//! - SGA (3), TERMINAL-TYPE (24, RFC 1091), NAWS (31, RFC 1073) and
//!   NEW-ENVIRON (39, RFC 1572) are offered *by us*. TERMINAL-TYPE
//!   answers `SEND` with the configured `term`; NAWS reports the
//!   viewport on enable and on every resize; NEW-ENVIRON answers with
//!   `USER` when a username is configured (how classic clients
//!   pre-fill the login prompt without scraping it).
//!
//! Everything else is declined, including BINARY, so NVT rules always
//! hold: inbound `CR NUL` collapses to `CR`, and `encode_input` maps
//! the terminal's bare `CR` to the `CR LF` new-line the protocol
//! expects from an Enter key.

pub(crate) const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const AYT: u8 = 246;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;
const OPT_NEW_ENVIRON: u8 = 39;

// TERMINAL-TYPE subnegotiation verbs (RFC 1091).
const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

// NEW-ENVIRON verbs and markers (RFC 1572).
const ENV_IS: u8 = 0;
const ENV_SEND: u8 = 1;
const ENV_VAR: u8 = 0;
const ENV_VALUE: u8 = 1;
const ENV_ESC: u8 = 2;
const ENV_USERVAR: u8 = 3;

/// RFC 1143 per-option state, one instance per (option, side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum QState {
    #[default]
    No,
    Yes,
    /// We asked to disable and wait for the acknowledgement.
    WantNo,
    /// As `WantNo`, but a new enable request arrived meanwhile. Only
    /// constructible once a mid-session renegotiation API exists; kept
    /// so the transition table below is the verbatim RFC 1143 machine
    /// rather than a subset that would silently loop if extended.
    #[allow(dead_code)]
    WantNoOpposite,
    /// We asked to enable and wait for the acknowledgement.
    WantYes,
    /// As `WantYes`, but a new disable request arrived meanwhile. Same
    /// status as `WantNoOpposite`.
    #[allow(dead_code)]
    WantYesOpposite,
}

/// Byte-stream parser position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Data,
    /// Consumed an IAC, next byte is a command.
    Iac,
    /// Consumed IAC + WILL/WONT/DO/DONT, next byte is the option.
    Command(u8),
    /// Consumed IAC SB, next byte is the subnegotiation option.
    SubnegOption,
    /// Accumulating subnegotiation payload bytes.
    SubnegData,
    /// Consumed an IAC inside a subnegotiation payload.
    SubnegIac,
}

/// Result of feeding one inbound chunk through the negotiator.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Decoded application bytes for the terminal.
    pub app: Vec<u8>,
    /// Protocol bytes that must be written back to the server verbatim.
    pub wire: Vec<u8>,
}

pub struct Negotiator {
    /// Option state for our side (server sends DO/DONT about these).
    us: [QState; 256],
    /// Option state for the server's side (server sends WILL/WONT).
    him: [QState; 256],
    state: ParseState,
    subneg_option: u8,
    subneg_buf: Vec<u8>,
    /// Inbound NVT line discipline: a CR was the last data byte seen,
    /// so a following NUL is padding to swallow (RFC 854 `CR NUL`).
    last_was_cr: bool,
    term: String,
    username: Option<String>,
    cols: u16,
    rows: u16,
}

impl Negotiator {
    /// Build the negotiator plus the proactive greeting a client sends
    /// on connect (the PuTTY-style opener): offer TERMINAL-TYPE, NAWS,
    /// NEW-ENVIRON and SGA on our side, request ECHO and SGA from the
    /// server. Every request is tracked as `WantYes`, so the server's
    /// answer settles the state instead of triggering a re-reply.
    pub fn new(term: &str, username: Option<&str>) -> (Self, Vec<u8>) {
        let mut neg = Negotiator {
            us: [QState::No; 256],
            him: [QState::No; 256],
            state: ParseState::Data,
            subneg_option: 0,
            subneg_buf: Vec::new(),
            last_was_cr: false,
            term: term.to_string(),
            username: username.map(str::to_string),
            cols: 80,
            rows: 24,
        };
        let mut greeting = Vec::new();
        for opt in [OPT_TTYPE, OPT_NAWS, OPT_NEW_ENVIRON, OPT_SGA] {
            neg.us[opt as usize] = QState::WantYes;
            greeting.extend_from_slice(&[IAC, WILL, opt]);
        }
        for opt in [OPT_ECHO, OPT_SGA] {
            neg.him[opt as usize] = QState::WantYes;
            greeting.extend_from_slice(&[IAC, DO, opt]);
        }
        (neg, greeting)
    }

    /// Options we agree to enable on our side when the server asks.
    fn we_support(option: u8) -> bool {
        matches!(option, OPT_SGA | OPT_TTYPE | OPT_NAWS | OPT_NEW_ENVIRON)
    }

    /// Options we want (or accept) the server to enable on its side.
    fn they_support(option: u8) -> bool {
        matches!(option, OPT_ECHO | OPT_SGA)
    }

    /// Feed inbound bytes; returns decoded app data and wire replies.
    pub fn receive(&mut self, input: &[u8]) -> Step {
        let mut step = Step::default();
        for &byte in input {
            match self.state {
                ParseState::Data => match byte {
                    IAC => self.state = ParseState::Iac,
                    0 if self.last_was_cr => {
                        // CR NUL is the NVT encoding of a bare carriage
                        // return; the CR already went out, drop the pad.
                        self.last_was_cr = false;
                    }
                    _ => {
                        self.last_was_cr = byte == b'\r';
                        step.app.push(byte);
                    }
                },
                ParseState::Iac => match byte {
                    // IAC IAC is a literal 0xFF data byte.
                    IAC => {
                        self.last_was_cr = false;
                        step.app.push(IAC);
                        self.state = ParseState::Data;
                    }
                    WILL | WONT | DO | DONT => self.state = ParseState::Command(byte),
                    SB => self.state = ParseState::SubnegOption,
                    AYT => {
                        // RFC 854 asks for "some visible evidence" that
                        // the far end is alive.
                        step.wire.extend_from_slice(b"\r\n[oryxis: yes]\r\n");
                        self.state = ParseState::Data;
                    }
                    // NOP, GA, DM, BRK, IP, AO, EC, EL: nothing to do.
                    _ => self.state = ParseState::Data,
                },
                ParseState::Command(verb) => {
                    self.handle_negotiation(verb, byte, &mut step.wire);
                    self.state = ParseState::Data;
                }
                ParseState::SubnegOption => {
                    self.subneg_option = byte;
                    self.subneg_buf.clear();
                    self.state = ParseState::SubnegData;
                }
                ParseState::SubnegData => match byte {
                    IAC => self.state = ParseState::SubnegIac,
                    _ => self.subneg_buf.push(byte),
                },
                ParseState::SubnegIac => match byte {
                    // IAC IAC inside a subnegotiation is a literal 0xFF.
                    IAC => {
                        self.subneg_buf.push(IAC);
                        self.state = ParseState::SubnegData;
                    }
                    SE => {
                        self.handle_subnegotiation(&mut step.wire);
                        self.state = ParseState::Data;
                    }
                    // Malformed: IAC inside SB followed by neither IAC
                    // nor SE. Be liberal: abandon the subnegotiation and
                    // reinterpret the byte as a fresh IAC command.
                    WILL | WONT | DO | DONT => {
                        self.subneg_buf.clear();
                        self.state = ParseState::Command(byte);
                    }
                    _ => {
                        self.subneg_buf.clear();
                        self.state = ParseState::Data;
                    }
                },
            }
        }
        step
    }

    /// RFC 1143 Q-method transition for one WILL/WONT/DO/DONT.
    fn handle_negotiation(&mut self, verb: u8, option: u8, wire: &mut Vec<u8>) {
        let i = option as usize;
        match verb {
            WILL => match self.him[i] {
                QState::No => {
                    if Self::they_support(option) {
                        self.him[i] = QState::Yes;
                        wire.extend_from_slice(&[IAC, DO, option]);
                    } else {
                        wire.extend_from_slice(&[IAC, DONT, option]);
                    }
                }
                QState::Yes => {}
                // "DONT answered by WILL" is a peer error; RFC 1143 says
                // treat the option as settled off, no further reply.
                QState::WantNo => self.him[i] = QState::No,
                QState::WantNoOpposite => self.him[i] = QState::Yes,
                QState::WantYes => self.him[i] = QState::Yes,
                QState::WantYesOpposite => {
                    self.him[i] = QState::WantNo;
                    wire.extend_from_slice(&[IAC, DONT, option]);
                }
            },
            WONT => match self.him[i] {
                QState::No => {}
                QState::Yes => {
                    self.him[i] = QState::No;
                    wire.extend_from_slice(&[IAC, DONT, option]);
                }
                QState::WantNo => self.him[i] = QState::No,
                QState::WantNoOpposite => {
                    self.him[i] = QState::WantYes;
                    wire.extend_from_slice(&[IAC, DO, option]);
                }
                QState::WantYes | QState::WantYesOpposite => self.him[i] = QState::No,
            },
            DO => match self.us[i] {
                QState::No => {
                    if Self::we_support(option) {
                        self.us[i] = QState::Yes;
                        wire.extend_from_slice(&[IAC, WILL, option]);
                        self.on_local_enabled(option, wire);
                    } else {
                        wire.extend_from_slice(&[IAC, WONT, option]);
                    }
                }
                QState::Yes => {}
                QState::WantNo => self.us[i] = QState::No,
                QState::WantNoOpposite => {
                    self.us[i] = QState::Yes;
                    self.on_local_enabled(option, wire);
                }
                QState::WantYes => {
                    self.us[i] = QState::Yes;
                    self.on_local_enabled(option, wire);
                }
                QState::WantYesOpposite => {
                    self.us[i] = QState::WantNo;
                    wire.extend_from_slice(&[IAC, WONT, option]);
                }
            },
            DONT => match self.us[i] {
                QState::No => {}
                QState::Yes => {
                    self.us[i] = QState::No;
                    wire.extend_from_slice(&[IAC, WONT, option]);
                }
                QState::WantNo => self.us[i] = QState::No,
                QState::WantNoOpposite => {
                    self.us[i] = QState::WantYes;
                    wire.extend_from_slice(&[IAC, WILL, option]);
                }
                QState::WantYes | QState::WantYesOpposite => self.us[i] = QState::No,
            },
            _ => unreachable!("only WILL/WONT/DO/DONT reach here"),
        }
    }

    /// Side effects of one of *our* options turning on.
    fn on_local_enabled(&mut self, option: u8, wire: &mut Vec<u8>) {
        if option == OPT_NAWS {
            // RFC 1073: the client reports the window size immediately
            // after the option is enabled, then again on every change.
            wire.extend_from_slice(&self.naws_packet());
        }
    }

    fn handle_subnegotiation(&mut self, wire: &mut Vec<u8>) {
        match self.subneg_option {
            OPT_TTYPE if self.subneg_buf.first() == Some(&TTYPE_SEND) => {
                // RFC 1091 lets the server cycle SEND to enumerate
                // types; with a single type we answer the same value
                // every time, which doubles as the end-of-list signal.
                let mut body = vec![TTYPE_IS];
                body.extend_from_slice(self.term.as_bytes());
                wire.extend_from_slice(&subnegotiation(OPT_TTYPE, &body));
            }
            OPT_NEW_ENVIRON if self.subneg_buf.first() == Some(&ENV_SEND) => {
                // Answer with USER when configured, else an empty IS.
                // The RFC 1572 markers (VAR/VALUE/ESC/USERVAR) must be
                // ESC-prefixed inside values; IAC doubling happens in
                // `subnegotiation`.
                let mut body = vec![ENV_IS];
                if let Some(user) = &self.username {
                    body.push(ENV_VAR);
                    body.extend_from_slice(b"USER");
                    body.push(ENV_VALUE);
                    for &b in user.as_bytes() {
                        if matches!(b, ENV_VAR | ENV_VALUE | ENV_ESC | ENV_USERVAR) {
                            body.push(ENV_ESC);
                        }
                        body.push(b);
                    }
                }
                wire.extend_from_slice(&subnegotiation(OPT_NEW_ENVIRON, &body));
            }
            // NAWS flows client -> server only; anything else is an
            // option we never enabled. Drop silently.
            _ => {}
        }
        self.subneg_buf.clear();
    }

    /// Record a viewport change; returns the NAWS report to send when
    /// the option is active (RFC 1073), `None` while it isn't (the size
    /// still updates, and the enable-time report picks it up).
    pub fn set_window(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        (self.us[OPT_NAWS as usize] == QState::Yes).then(|| self.naws_packet())
    }

    fn naws_packet(&self) -> Vec<u8> {
        let [ch, cl] = self.cols.to_be_bytes();
        let [rh, rl] = self.rows.to_be_bytes();
        subnegotiation(OPT_NAWS, &[ch, cl, rh, rl])
    }
}

/// Wrap a subnegotiation body as `IAC SB <option> <body> IAC SE`,
/// doubling any 0xFF byte inside the body.
fn subnegotiation(option: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![IAC, SB, option];
    for &b in body {
        if b == IAC {
            out.push(IAC);
        }
        out.push(b);
    }
    out.extend_from_slice(&[IAC, SE]);
    out
}

/// Encode terminal input for the wire: double IAC bytes and map the
/// Enter key's bare CR to the protocol's CR LF new line (what line-mode
/// servers and network appliances expect; PuTTY's default too). A CR LF
/// already present in the input stays a single CR LF.
pub fn encode_input(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            IAC => out.extend_from_slice(&[IAC, IAC]),
            b'\r' => {
                out.extend_from_slice(b"\r\n");
                // Collapse an explicit CR LF pair into one new line.
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_offers_and_requests_the_expected_options() {
        let (_, greeting) = Negotiator::new("xterm-256color", None);
        for opt in [OPT_TTYPE, OPT_NAWS, OPT_NEW_ENVIRON, OPT_SGA] {
            assert!(
                greeting.windows(3).any(|w| w == [IAC, WILL, opt]),
                "greeting missing WILL {opt}"
            );
        }
        for opt in [OPT_ECHO, OPT_SGA] {
            assert!(
                greeting.windows(3).any(|w| w == [IAC, DO, opt]),
                "greeting missing DO {opt}"
            );
        }
    }

    #[test]
    fn requested_options_settle_without_re_reply() {
        // The greeting already asked WILL TTYPE / DO ECHO; the server's
        // answer must settle the state silently. A naive implementation
        // replies again here, which is exactly the RFC 1143 loop.
        let (mut neg, _) = Negotiator::new("xterm", None);
        let step = neg.receive(&[IAC, DO, OPT_TTYPE, IAC, WILL, OPT_ECHO]);
        assert!(step.wire.is_empty(), "unexpected reply: {:?}", step.wire);
        // Re-announcing an already-on option stays silent too.
        let step = neg.receive(&[IAC, WILL, OPT_ECHO]);
        assert!(step.wire.is_empty());
    }

    #[test]
    fn unsupported_options_are_declined() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        // LINEMODE (34) on our side, STATUS (5) on the server's.
        let step = neg.receive(&[IAC, DO, 34, IAC, WILL, 5]);
        assert_eq!(step.wire, vec![IAC, WONT, 34, IAC, DONT, 5]);
    }

    #[test]
    fn declined_requests_settle_to_off() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        // Server refuses everything we asked for in the greeting.
        let step = neg.receive(&[
            IAC, DONT, OPT_TTYPE, IAC, DONT, OPT_NAWS, IAC, DONT, OPT_NEW_ENVIRON, IAC, DONT,
            OPT_SGA, IAC, WONT, OPT_ECHO, IAC, WONT, OPT_SGA,
        ]);
        assert!(step.wire.is_empty(), "refusals must not be answered");
        // NAWS never enabled, so a resize produces no packet.
        assert_eq!(neg.set_window(120, 40), None);
    }

    #[test]
    fn ttype_send_answers_with_the_configured_term() {
        let (mut neg, _) = Negotiator::new("xterm-256color", None);
        neg.receive(&[IAC, DO, OPT_TTYPE]);
        let step = neg.receive(&[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE]);
        let mut expected = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        expected.extend_from_slice(b"xterm-256color");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(step.wire, expected);
    }

    #[test]
    fn new_environ_send_answers_with_user() {
        let (mut neg, _) = Negotiator::new("xterm", Some("admin"));
        neg.receive(&[IAC, DO, OPT_NEW_ENVIRON]);
        let step = neg.receive(&[IAC, SB, OPT_NEW_ENVIRON, ENV_SEND, IAC, SE]);
        let mut expected = vec![IAC, SB, OPT_NEW_ENVIRON, ENV_IS, ENV_VAR];
        expected.extend_from_slice(b"USER");
        expected.push(ENV_VALUE);
        expected.extend_from_slice(b"admin");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(step.wire, expected);
    }

    #[test]
    fn new_environ_without_username_answers_empty_is() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        neg.receive(&[IAC, DO, OPT_NEW_ENVIRON]);
        let step = neg.receive(&[IAC, SB, OPT_NEW_ENVIRON, ENV_SEND, IAC, SE]);
        assert_eq!(step.wire, vec![IAC, SB, OPT_NEW_ENVIRON, ENV_IS, IAC, SE]);
    }

    #[test]
    fn naws_reports_on_enable_and_on_resize() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        let step = neg.receive(&[IAC, DO, OPT_NAWS]);
        // Enable-time report carries the default 80x24 until told better.
        assert_eq!(
            step.wire,
            vec![IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]
        );
        assert_eq!(
            neg.set_window(120, 40),
            Some(vec![IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE])
        );
    }

    #[test]
    fn naws_packet_doubles_iac_bytes() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        neg.receive(&[IAC, DO, OPT_NAWS]);
        // 255 columns puts a literal 0xFF inside the payload.
        let packet = neg.set_window(255, 24).unwrap();
        assert_eq!(
            packet,
            vec![IAC, SB, OPT_NAWS, 0, IAC, IAC, 0, 24, IAC, SE]
        );
    }

    #[test]
    fn data_passes_through_with_iac_sequences_stripped() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        // "ab" + NOP + "cd" + escaped literal 0xFF + "e"
        let step = neg.receive(&[b'a', b'b', IAC, 241, b'c', b'd', IAC, IAC, b'e']);
        assert_eq!(step.app, vec![b'a', b'b', b'c', b'd', IAC, b'e']);
    }

    #[test]
    fn cr_nul_collapses_even_across_chunks() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        let step = neg.receive(b"a\r\0b\r\nc");
        assert_eq!(step.app, b"a\rb\r\nc".to_vec());
        // The CR NUL pair split across two reads must still collapse.
        let step = neg.receive(b"x\r");
        assert_eq!(step.app, b"x\r".to_vec());
        let step = neg.receive(b"\0y");
        assert_eq!(step.app, b"y".to_vec());
    }

    #[test]
    fn subnegotiation_survives_chunk_splits_and_embedded_iac() {
        let (mut neg, _) = Negotiator::new("xterm-256color", None);
        neg.receive(&[IAC, DO, OPT_TTYPE]);
        // Split the SB across three reads; an IAC IAC pair inside the
        // payload must not terminate it early.
        let step1 = neg.receive(&[IAC, SB, OPT_TTYPE]);
        assert!(step1.wire.is_empty());
        let step2 = neg.receive(&[TTYPE_SEND, IAC]);
        assert!(step2.wire.is_empty());
        let step3 = neg.receive(&[SE]);
        assert!(!step3.wire.is_empty());
        assert_eq!(step3.wire[3], TTYPE_IS);
    }

    #[test]
    fn ayt_gets_visible_evidence() {
        let (mut neg, _) = Negotiator::new("xterm", None);
        let step = neg.receive(&[IAC, AYT]);
        assert!(!step.wire.is_empty());
        assert!(step.app.is_empty());
    }

    #[test]
    fn encode_input_maps_enter_and_doubles_iac() {
        assert_eq!(encode_input(b"ls\r"), b"ls\r\n".to_vec());
        // An explicit CR LF stays one new line, not two.
        assert_eq!(encode_input(b"ls\r\n"), b"ls\r\n".to_vec());
        assert_eq!(encode_input(&[0x41, IAC, 0x42]), vec![0x41, IAC, IAC, 0x42]);
        assert_eq!(encode_input(b"plain"), b"plain".to_vec());
    }
}
