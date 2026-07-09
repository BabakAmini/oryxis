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

/// Build automatic answers for one keyboard-interactive round, or `None`
/// when the round must go to the user. Succeeds only when a TOTP
/// generator is configured, at least one prompt is an OTP ask, and every
/// prompt in the round is answerable (OTP prompts get a generated code,
/// password prompts get the stored password). Any unrecognized prompt
/// surfaces the whole round, guessing at unknown prompts would burn
/// server-side auth attempts.
pub(crate) fn autofill_kbi_round<'a>(
    totp: Option<&oryxis_core::totp::Totp>,
    prompts: impl IntoIterator<Item = &'a str>,
    fallback_pw: Option<&str>,
) -> Option<Vec<String>> {
    let totp = totp?;
    let mut answers = Vec::new();
    let mut any_otp = false;
    for prompt in prompts {
        if prompt_wants_otp(prompt) {
            any_otp = true;
            answers.push(totp.code_now());
        } else if prompt.to_lowercase().contains("password") {
            answers.push(fallback_pw?.to_string());
        } else {
            return None;
        }
    }
    (any_otp && !answers.is_empty()).then_some(answers)
}
