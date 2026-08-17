//! Pre-pay the OS file dialog's first-open cost, in the background.
//!
//! `rfd` builds every dialog from scratch on a fresh thread: COM
//! apartment, `CoCreateInstance(FileSaveDialog)`, then `Show`. The
//! apartment and the thread are microseconds; what the FIRST dialog in a
//! process really pays for is loading the shell's COM server and every
//! namespace extension registered on the machine (cloud providers,
//! archivers, AV shells). That shows up as "the Download menu item took
//! a while to open the picker" and nothing else, because the app itself
//! does no work before the dialog: the SFTP channel pool is only built
//! once a destination exists.
//!
//! So one dialog object is created at boot on a background thread and
//! dropped without ever being shown. Nothing is user-visible, and a
//! failure is silent by design: this warms a cache, it is not a feature,
//! and a machine where it fails simply pays the cost later, as it does
//! today.
//!
//! Deliberately NOT warmed: the folder the picker will open on, which
//! `SHCreateItemFromParsingName` would resolve. That call is only
//! expensive when the folder is remote (a mapped drive, a UNC path, a
//! disconnected cloud mount) and that is exactly the case where it
//! blocks for the network's timeout rather than milliseconds. Doing it
//! here would move a stall from "when the user opens a dialog" to "every
//! boot, whether or not they ever do", and for a local folder, which is
//! nearly all of them, it saves nothing measurable.
//!
//! Windows only. macOS's `NSSavePanel` and the XDG portal have their own
//! first-call cost, but no equivalent create-without-showing hook: the
//! portal one is a D-Bus round trip that already spawns the picker.

/// Warm the OS file dialog up. Returns immediately; the work runs on a
/// detached thread. No-op off Windows.
#[cfg(target_os = "windows")]
pub(crate) fn spawn() {
    std::thread::spawn(imp::warm);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn spawn() {}

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog};

    pub(super) fn warm() {
        unsafe {
            // The same apartment model rfd initializes on its own dialog
            // thread. A different one would warm a different marshalling
            // path than the real call takes.
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_err() {
                return;
            }
            // Never shown: creating it is what pulls in the shell's COM
            // server and the registered namespace extensions.
            let dialog: windows::core::Result<IFileSaveDialog> =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER);
            if let Err(e) = &dialog {
                tracing::debug!("file dialog warm-up: create failed: {e}");
            }
            drop(dialog);
            CoUninitialize();
        }
    }
}
