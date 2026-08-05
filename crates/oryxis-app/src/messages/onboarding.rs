//! First-run welcome / onboarding carousel messages, wrapped by
//! [`crate::messages::Message::Onboarding`]. Handled by
//! `Oryxis::handle_onboarding`.

/// Drives the slide index of the onboarding carousel (rendered off
/// `VaultState::NeedSetup`); the final slide creates the vault via the
/// existing `Vault*` messages.
#[derive(Debug, Clone)]
pub enum OnboardingMessage {
    /// Advance one slide (clamped to the last).
    Next,
    /// Step back one slide (clamped to zero).
    Back,
    /// Jump straight to the final (password-setup) slide.
    SkipToEnd,
    /// "Import my hosts" on the import slide: remembers the intent
    /// (the vault does not exist yet) and jumps to the final slide.
    ImportAfterSetup,
}
