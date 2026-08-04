/// True when a keyboard-interactive prompt is asking for a one-time
/// code. Matched against the lowercased prompt; the list covers the
/// stock PAM modules (google-authenticator "Verification code:",
/// pam_oath "One-time password ...", Duo "Passcode or option ...") and
/// the generic phrasings commercial MFA gateways use.
pub(crate) fn prompt_wants_otp(prompt: &str) -> bool {
    let p = prompt.to_lowercase();
    [
        "verification code",
        "one-time",
        "one time",
        "otp",
        "authenticator",
        "passcode",
        "security code",
        "2fa",
        "mfa",
        "second factor",
    ]
    .iter()
    .any(|k| p.contains(k))
}

/// True when the round-level `name` / `instructions` of a
/// keyboard-interactive exchange mark the whole round as an OTP ask.
/// Some servers put the operative words there and keep the prompt
/// itself terse: Bitvise sends name "TOTP authentication",
/// instructions "Get a time-based one time password from your
/// authenticator." and a bare "(account) Enter code:" prompt
/// (issue #125).
pub(crate) fn round_context_wants_otp(name: &str, instructions: &str) -> bool {
    prompt_wants_otp(name) || prompt_wants_otp(instructions)
}

/// Build automatic answers for one keyboard-interactive round, or `None`
/// when the round must go to the user. Succeeds only when a TOTP
/// generator is configured, at least one prompt is an OTP ask, and every
/// prompt in the round is answerable (OTP prompts get a generated code,
/// password prompts get the stored password). Any unrecognized prompt
/// surfaces the whole round, guessing at unknown prompts would burn
/// server-side auth attempts.
///
/// `context_wants_otp` (see `round_context_wants_otp`) widens the
/// per-prompt match: when the round announces itself as an OTP
/// exchange, a prompt asking for a "code" or "token" counts as an OTP
/// prompt even though the prompt text alone carries no OTP keyword.
pub(crate) fn autofill_kbi_round<'a>(
    totp: Option<&oryxis_core::totp::Totp>,
    context_wants_otp: bool,
    prompts: impl IntoIterator<Item = &'a str>,
    fallback_pw: Option<&str>,
) -> Option<Vec<String>> {
    let totp = totp?;
    let mut answers = Vec::new();
    let mut any_otp = false;
    for prompt in prompts {
        let p = prompt.to_lowercase();
        if prompt_wants_otp(prompt)
            || (context_wants_otp && (p.contains("code") || p.contains("token")))
        {
            any_otp = true;
            answers.push(totp.code_now());
        } else if p.contains("password") {
            answers.push(fallback_pw?.to_string());
        } else {
            return None;
        }
    }
    (any_otp && !answers.is_empty()).then_some(answers)
}
