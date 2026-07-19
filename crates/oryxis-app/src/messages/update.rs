//! Auto-update check / download / channel arms, wrapped by [`crate::messages::Message::Update`]. Handled by `Oryxis::handle_update`.

#[derive(Debug, Clone)]
pub enum UpdateMessage {
    /// Settings: switch the auto-update release channel (stable/nightly).
    SettingUpdateChannelChanged(crate::update::UpdateChannel),
    SettingToggleAutoCheckUpdates,
    CheckForUpdate,
    CheckForUpdateManual,
    UpdateCheckResult(Option<crate::update::UpdateInfo>),
    /// Manual update check failed (network / HTTP / parse); carries the
    /// concise cause for the Settings > About status line + toast.
    UpdateCheckFailed(String),
    UpdateSkipVersion,
    UpdateLater,
    UpdateStartDownload,
    UpdateDownloadProgress(f32),
    UpdateDownloadComplete(Result<std::path::PathBuf, String>),
    UpdateOpenRelease,
}
