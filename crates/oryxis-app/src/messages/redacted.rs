//! [`Redacted`], the payload type for every message field that carries a
//! secret.
//!
//! Messages derive `Debug`, and two places format a whole message with
//! it: the stall watchdog's ring (`stall_watchdog::message_name`, whose
//! report is written into the debug-log file users attach to issues) and
//! `dispatch::unrouted`. A `String` payload therefore reaches disk, and
//! since a text-input message carries the field's value on EVERY
//! keystroke, the ring ends up holding the complete password rather than
//! a fragment.
//!
//! Wrapping the payload fixes the class at the source instead of at each
//! sink: `Debug` stays derived everywhere, the compiler points at every
//! construction and read site, and any future `{message:?}` is safe by
//! construction. Reading the secret is spelled `expose()` /
//! `into_inner()` so a call site that leaks one has to say so.

/// A string payload that must never reach a log.
///
/// `Debug` prints `<redacted>`; the value comes out only through
/// [`Redacted::into_inner`], which is deliberately the ONLY accessor: no
/// `Deref`, no `as_str`, so unwrapping a secret is always a visible act
/// at the call site.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Redacted(String);

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Redacted {
    /// Take ownership of the secret. Every handler here does the same
    /// thing with it (move it into the form / app state), so one
    /// consuming accessor covers the set.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for Redacted {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Redacted {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Redacted;

    /// The whole point: neither the direct `Debug` nor a `Debug` reached
    /// through an enclosing type may print the secret.
    #[test]
    fn debug_never_prints_the_secret() {
        let r = Redacted::from("hunter2".to_string());
        assert_eq!(format!("{r:?}"), "<redacted>");

        #[derive(Debug)]
        #[allow(dead_code)]
        enum Wrapper {
            PasswordChanged(Redacted),
        }
        let printed = format!("{:?}", Wrapper::PasswordChanged(r.clone()));
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(!printed.contains("hunter2"), "{printed}");

        // And the value is still reachable for the handler that needs it.
        assert_eq!(r.into_inner(), "hunter2");
    }

    /// Structural guard: a secret-bearing message variant must carry
    /// `Redacted`, never a bare `String`. Adding
    /// `SomethingPasswordChanged(String)` to any sub-enum fails here,
    /// which is the only thing standing between a new field and the
    /// debug log (`stall_watchdog::message_name` formats whatever the
    /// message's `Debug` prints).
    #[test]
    fn secret_bearing_variants_carry_redacted() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("messages");
        // Names that mean "a secret rides in this payload". `Key` is not
        // on the list on purpose: it also names key SELECTION variants
        // (`EditorKeyChanged` is a saved-key id), so it would fire on
        // things that carry no secret at all.
        const SECRET: [&str; 6] = [
            "Password", "Passphrase", "ApiKey", "Secret", "Totp", "KbiInput",
        ];
        let mut offenders: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("messages dir").flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let trimmed = line.trim();
                // Variant declarations only: `Name(payload),` at the top
                // of a line inside an enum body.
                let Some((name, payload)) = trimmed.split_once('(') else { continue };
                if !payload.ends_with("),") || !name.chars().all(|c| c.is_alphanumeric()) {
                    continue;
                }
                if !SECRET.iter().any(|s| name.contains(s)) {
                    continue;
                }
                if payload.contains("String") {
                    offenders.push(format!(
                        "{}: {trimmed}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "secret-bearing variants must carry `Redacted`, not `String`, \
             or the payload reaches the debug log through `Debug`:\n{}",
            offenders.join("\n")
        );
    }
}
