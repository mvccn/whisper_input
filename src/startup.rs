//! Startup diagnostics for single-instance enforcement and macOS permission prompts.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{info, warn};

/// Process-lifetime guard that keeps the single-instance lock held.
pub(crate) struct SingleInstanceGuard {
    _file: File,
}

/// Attempts to acquire the single-instance lock for the current user session.
///
/// # Intent
/// Prevents duplicate menu-bar instances when the app is launched both manually
/// and via auto-start at login.
///
/// # Usage
/// ```no_run
/// # use anyhow::Result;
/// # fn demo() -> Result<()> {
/// let guard = whisper_input_startup_example();
/// # let _ = guard;
/// # Ok(())
/// # }
/// # fn whisper_input_startup_example() -> Option<()> { None }
/// ```
///
/// # Errors
/// Returns an error if the lock file cannot be created or updated.
pub(crate) fn acquire_single_instance() -> Result<Option<SingleInstanceGuard>> {
    let lock_path = default_lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create lock directory at {}", parent.display()))?;
    }

    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file at {}", lock_path.display()))?;

    if !try_lock_file(&file)? {
        info!(path = %lock_path.display(), "another whisper_input instance is already running");
        return Ok(None);
    }

    write_lock_metadata(&file).context("failed to update lock file metadata")?;
    Ok(Some(SingleInstanceGuard { _file: file }))
}

/// Runs one-time startup diagnostics and prompts for missing macOS permissions.
///
/// # Intent
/// Surfaces missing permissions as soon as the login-start tray app launches,
/// rather than waiting for the user to discover broken hotkeys or paste later.
///
/// # Usage
/// ```no_run
/// whisper_input_startup_checks_example();
/// # fn whisper_input_startup_checks_example() {}
/// ```
pub(crate) fn run_startup_checks() {
    #[cfg(target_os = "macos")]
    if let Err(err) = run_macos_startup_checks() {
        warn!(error = %err, "startup diagnostics failed");
    }
}

/// Shows an on-demand permission diagnostics dialog for the current runtime.
///
/// # Intent
/// Lets users re-check the macOS permissions that control the global hotkey,
/// microphone capture, and synthetic paste after launch.
///
/// # Usage
/// Called from the tray `Diagnose Permissions...` menu item.
///
/// # Errors
/// Returns an error if runtime-path resolution or dialog presentation fails.
pub(crate) fn show_permission_diagnostics_dialog() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_macos_permission_diagnostics_dialog()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// Returns the per-user lock file location.
fn default_lock_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("whisper_input")
        .join("instance.lock")
}

/// Writes lightweight diagnostic metadata into the lock file.
///
/// # Errors
/// Returns an error if the lock file cannot be rewritten.
fn write_lock_metadata(file: &File) -> Result<()> {
    file.set_len(0).context("failed to truncate lock file")?;
    let mut writer = file;
    writer
        .seek(SeekFrom::Start(0))
        .context("failed to rewind lock file")?;
    writeln!(writer, "pid={}", std::process::id()).context("failed to write process id")?;
    writer.flush().context("failed to flush lock metadata")?;
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use block2::{Block, RcBlock};
    use core_foundation_sys::base::{CFRelease, kCFAllocatorDefault};
    use core_foundation_sys::dictionary::{
        CFDictionaryCreate, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
        kCFTypeDictionaryValueCallBacks,
    };
    use core_foundation_sys::number::kCFBooleanTrue;
    use core_foundation_sys::string::CFStringRef;
    use handy_keys::check_accessibility;
    use objc2::runtime::Bool as ObjcBool;
    use tracing::{info, warn};

    use super::try_lock_file_result;

    const LOCK_EXCLUSIVE: i32 = 0x02;
    const LOCK_NONBLOCK: i32 = 0x04;
    const SETTINGS_PRIVACY_GENERAL_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security";
    const SETTINGS_ACCESSIBILITY_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
    const SETTINGS_INPUT_MONITORING_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";
    const SETTINGS_MICROPHONE_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum PermissionIssue {
        Accessibility,
        InputMonitoring,
        Microphone,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LaunchContext {
        current_executable_path: PathBuf,
        current_app_bundle_path: Option<PathBuf>,
        expected_login_app_path: Option<PathBuf>,
    }

    impl LaunchContext {
        /// Resolves the current runtime paths used for startup diagnostics.
        ///
        /// # Errors
        /// Returns an error if the current executable path cannot be determined.
        fn resolve() -> Result<Self> {
            let current_executable_path =
                std::env::current_exe().context("failed to resolve current executable path")?;
            let current_app_bundle_path = app_bundle_path_for_executable(&current_executable_path);
            let expected_login_app_path =
                std::env::var_os("WHISPER_EXPECTED_APP_PATH").map(PathBuf::from);

            Ok(Self {
                current_executable_path,
                current_app_bundle_path,
                expected_login_app_path,
            })
        }

        /// Returns the path macOS permission checks apply to for this process.
        fn permission_target_path(&self) -> &Path {
            self.current_app_bundle_path
                .as_deref()
                .unwrap_or(self.current_executable_path.as_path())
        }

        /// Returns whether the current runtime matches the expected login app.
        fn login_target_matches_current_runtime(&self) -> Option<bool> {
            self.expected_login_app_path.as_ref().map(|expected| {
                self.current_app_bundle_path
                    .as_ref()
                    .is_some_and(|current| current == expected)
            })
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct PermissionReport {
        launch_context: LaunchContext,
        accessibility_granted: bool,
        input_monitoring_granted: bool,
        microphone_granted: bool,
        issues: Vec<PermissionIssue>,
    }

    impl PermissionReport {
        /// Returns true when the report still needs user attention.
        fn needs_attention(&self) -> bool {
            !self.issues.is_empty()
                || self.launch_context.login_target_matches_current_runtime() == Some(false)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DialogAction {
        OpenSettings,
        Later,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MicrophoneAuthorizationStatus {
        NotDetermined,
        Restricted,
        Denied,
        Authorized,
        Unknown(isize),
    }

    impl MicrophoneAuthorizationStatus {
        /// Converts the AVFoundation raw status code to a typed status.
        fn from_raw(raw: isize) -> Self {
            match raw {
                0 => Self::NotDetermined,
                1 => Self::Restricted,
                2 => Self::Denied,
                3 => Self::Authorized,
                other => Self::Unknown(other),
            }
        }
    }

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: *mut c_void;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
        fn CGPreflightPostEventAccess() -> bool;
        fn CGRequestPostEventAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const i8) -> *mut c_void;
        fn sel_registerName(name: *const i8) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    type AuthorizationStatusMsgSend =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> isize;
    type RequestAccessMsgSend =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const Block<dyn Fn(ObjcBool)>);

    /// Attempts a non-blocking exclusive lock on the provided file.
    ///
    /// # Errors
    /// Returns an error if the underlying `flock` call fails for a reason other
    /// than another process already holding the lock.
    pub(super) fn try_lock_file(file: &File) -> Result<bool> {
        let result = unsafe { flock(file.as_raw_fd(), LOCK_EXCLUSIVE | LOCK_NONBLOCK) };
        try_lock_file_result(result)
    }

    /// Runs the macOS startup permission checks and prompts once if needed.
    ///
    /// # Errors
    /// Returns an error if prompt invocation fails unexpectedly.
    pub(super) fn run_macos_startup_checks() -> Result<()> {
        let launch_context = LaunchContext::resolve()?;
        log_launch_context(&launch_context);

        let issues = collect_permission_issues_with_prompt()?;
        if issues.is_empty() {
            info!(
                permission_target = %launch_context.permission_target_path().display(),
                "startup permission check passed"
            );
            return Ok(());
        }

        warn!(
            issues = ?issues,
            permission_target = %launch_context.permission_target_path().display(),
            expected_login_app_path = ?launch_context.expected_login_app_path,
            matches_expected_login_app = ?launch_context.login_target_matches_current_runtime(),
            "startup permission check found missing permissions"
        );

        if show_permission_dialog(&issues, &launch_context)? == DialogAction::OpenSettings {
            open_system_settings(&issues)?;
        }

        Ok(())
    }

    /// Shows the current permission status without re-triggering native prompts.
    ///
    /// # Errors
    /// Returns an error if runtime-path resolution or dialog presentation fails.
    pub(super) fn run_macos_permission_diagnostics_dialog() -> Result<()> {
        let report = current_permission_report()?;
        if show_permission_status_dialog(&report)? == DialogAction::OpenSettings
            && report.needs_attention()
        {
            open_system_settings(&report.issues)?;
        }

        Ok(())
    }

    /// Maps raw permission booleans to the user-facing missing-permission list.
    fn permission_issues_from_grants(
        accessibility_granted: bool,
        input_monitoring_granted: bool,
        microphone_granted: bool,
    ) -> Vec<PermissionIssue> {
        let mut issues = Vec::new();

        if !accessibility_granted {
            issues.push(PermissionIssue::Accessibility);
        }
        if !input_monitoring_granted {
            issues.push(PermissionIssue::InputMonitoring);
        }
        if !microphone_granted {
            issues.push(PermissionIssue::Microphone);
        }

        issues
    }

    /// Collects all missing or incomplete permissions required by the app.
    fn collect_permission_issues() -> Vec<PermissionIssue> {
        permission_issues_from_grants(
            has_accessibility_access() && has_post_event_access(),
            has_input_monitoring_access(),
            microphone_authorization_status() == MicrophoneAuthorizationStatus::Authorized,
        )
    }

    /// Reads the current permission state for the running app path.
    ///
    /// # Errors
    /// Returns an error if the current runtime path cannot be resolved.
    fn current_permission_report() -> Result<PermissionReport> {
        let launch_context = LaunchContext::resolve()?;
        let accessibility_granted = has_accessibility_access() && has_post_event_access();
        let input_monitoring_granted = has_input_monitoring_access();
        let microphone_granted =
            microphone_authorization_status() == MicrophoneAuthorizationStatus::Authorized;
        let issues = permission_issues_from_grants(
            accessibility_granted,
            input_monitoring_granted,
            microphone_granted,
        );

        Ok(PermissionReport {
            launch_context,
            accessibility_granted,
            input_monitoring_granted,
            microphone_granted,
            issues,
        })
    }

    /// Requests native macOS permission prompts first, then returns anything
    /// that still needs manual resolution in System Settings.
    ///
    /// # Errors
    /// Returns an error if the microphone authorization request fails.
    fn collect_permission_issues_with_prompt() -> Result<Vec<PermissionIssue>> {
        if !has_accessibility_access() {
            request_accessibility_access();
        }

        if !has_input_monitoring_access() {
            let granted = request_input_monitoring_access();
            info!(granted, "requested input monitoring permission");
        }

        if !has_post_event_access() {
            let granted = request_post_event_access();
            info!(granted, "requested synthetic event permission");
        }

        if microphone_authorization_status() == MicrophoneAuthorizationStatus::NotDetermined {
            let granted = request_microphone_access()?;
            info!(granted, "requested microphone permission");
        }

        Ok(collect_permission_issues())
    }

    /// Returns true when Accessibility access is granted.
    fn has_accessibility_access() -> bool {
        check_accessibility()
    }

    /// Returns true when global keyboard-listening access is granted.
    fn has_input_monitoring_access() -> bool {
        unsafe { CGPreflightListenEventAccess() }
    }

    /// Returns true when posting synthetic input events is allowed.
    fn has_post_event_access() -> bool {
        unsafe { CGPreflightPostEventAccess() }
    }

    /// Triggers the macOS Input Monitoring prompt when the permission is absent.
    fn request_input_monitoring_access() -> bool {
        unsafe { CGRequestListenEventAccess() }
    }

    /// Triggers the macOS synthetic-event permission prompt when needed.
    fn request_post_event_access() -> bool {
        unsafe { CGRequestPostEventAccess() }
    }

    /// Triggers the macOS Accessibility trust prompt.
    fn request_accessibility_access() {
        let keys = [unsafe { kAXTrustedCheckOptionPrompt as *const c_void }];
        let values = [unsafe { kCFBooleanTrue as *const c_void }];
        let options = unsafe {
            CFDictionaryCreate(
                kCFAllocatorDefault,
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        };

        if options.is_null() {
            warn!("failed to construct accessibility prompt options dictionary");
            return;
        }

        let trusted = unsafe { AXIsProcessTrustedWithOptions(options) };
        unsafe { CFRelease(options.cast()) };
        info!(trusted, "requested accessibility permission");
    }

    /// Reads the current microphone authorization state from AVFoundation.
    fn microphone_authorization_status() -> MicrophoneAuthorizationStatus {
        let raw = unsafe { av_capture_device_authorization_status_for_audio() };
        MicrophoneAuthorizationStatus::from_raw(raw)
    }

    /// Requests microphone access and waits briefly for the user's response.
    ///
    /// # Errors
    /// Returns an error if the AVFoundation request cannot be dispatched.
    fn request_microphone_access() -> Result<bool> {
        let class = unsafe { objc_getClass(c"AVCaptureDevice".as_ptr()) };
        let selector =
            unsafe { sel_registerName(c"requestAccessForMediaType:completionHandler:".as_ptr()) };
        if class.is_null() || selector.is_null() || unsafe { AVMediaTypeAudio.is_null() } {
            return Ok(false);
        }

        let (tx, rx) = mpsc::channel();
        let block: RcBlock<dyn Fn(ObjcBool)> = RcBlock::new(move |granted: ObjcBool| {
            let _ = tx.send(granted.as_bool());
        });

        let msg_send: RequestAccessMsgSend =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe {
            msg_send(class, selector, AVMediaTypeAudio, &*block);
        }

        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(granted) => Ok(granted),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                info!("microphone authorization request timed out while waiting for user response");
                Ok(false)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
                "microphone authorization request channel disconnected"
            )),
        }
    }

    /// Calls `+[AVCaptureDevice authorizationStatusForMediaType:]` for audio.
    unsafe fn av_capture_device_authorization_status_for_audio() -> isize {
        let class = unsafe { objc_getClass(c"AVCaptureDevice".as_ptr()) };
        let selector = unsafe { sel_registerName(c"authorizationStatusForMediaType:".as_ptr()) };
        if class.is_null() || selector.is_null() || unsafe { AVMediaTypeAudio.is_null() } {
            return -1;
        }

        let msg_send: AuthorizationStatusMsgSend =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { msg_send(class, selector, AVMediaTypeAudio) }
    }

    /// Shows a one-time startup prompt summarizing missing permissions.
    ///
    /// # Errors
    /// Returns an error if `osascript` cannot be launched successfully.
    fn show_permission_dialog(
        issues: &[PermissionIssue],
        launch_context: &LaunchContext,
    ) -> Result<DialogAction> {
        let title = "WhisperInput Needs macOS Permissions";
        let message = build_permission_dialog_message(issues, launch_context);
        let output = Command::new("osascript")
            .arg("-e")
            .arg("on run argv")
            .arg("-e")
            .arg("set dialogText to item 1 of argv")
            .arg("-e")
            .arg("set dialogTitle to item 2 of argv")
            .arg("-e")
            .arg("display dialog dialogText with title dialogTitle buttons {\"Later\", \"Open Settings\"} default button \"Open Settings\" with icon caution")
            .arg("-e")
            .arg("button returned of result")
            .arg("-e")
            .arg("end run")
            .arg(message)
            .arg(title)
            .output()
            .context("failed to launch permission dialog")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(stderr = %stderr.trim(), "permission dialog did not complete normally");
            return Ok(DialogAction::Later);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim() == "Open Settings" {
            return Ok(DialogAction::OpenSettings);
        }

        Ok(DialogAction::Later)
    }

    /// Shows the current permission-status summary for the running app path.
    ///
    /// # Errors
    /// Returns an error if `osascript` cannot be launched successfully.
    fn show_permission_status_dialog(report: &PermissionReport) -> Result<DialogAction> {
        let title = "WhisperInput Permission Diagnostics";
        let message = build_permission_status_dialog_message(report);
        let mut command = Command::new("osascript");
        command
            .arg("-e")
            .arg("on run argv")
            .arg("-e")
            .arg("set dialogText to item 1 of argv")
            .arg("-e")
            .arg("set dialogTitle to item 2 of argv");

        if report.needs_attention() {
            command
                .arg("-e")
                .arg("display dialog dialogText with title dialogTitle buttons {\"Close\", \"Open Settings\"} default button \"Open Settings\" with icon caution");
        } else {
            command
                .arg("-e")
                .arg("display dialog dialogText with title dialogTitle buttons {\"OK\"} default button \"OK\" with icon note");
        }

        let output = command
            .arg("-e")
            .arg("button returned of result")
            .arg("-e")
            .arg("end run")
            .arg(message)
            .arg(title)
            .output()
            .context("failed to launch permission diagnostics dialog")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                stderr = %stderr.trim(),
                "permission diagnostics dialog did not complete normally"
            );
            return Ok(DialogAction::Later);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim() == "Open Settings" {
            return Ok(DialogAction::OpenSettings);
        }

        Ok(DialogAction::Later)
    }

    /// Logs the startup runtime path that permission checks apply to.
    fn log_launch_context(launch_context: &LaunchContext) {
        info!(
            current_executable_path = %launch_context.current_executable_path.display(),
            current_app_bundle_path = ?launch_context.current_app_bundle_path,
            expected_login_app_path = ?launch_context.expected_login_app_path,
            matches_expected_login_app = ?launch_context.login_target_matches_current_runtime(),
            permission_target = %launch_context.permission_target_path().display(),
            "resolved startup permission target"
        );

        if launch_context.login_target_matches_current_runtime() == Some(false) {
            warn!(
                current_app_bundle_path = ?launch_context.current_app_bundle_path,
                expected_login_app_path = ?launch_context.expected_login_app_path,
                "login-start runtime does not match the expected installed app bundle"
            );
        }
    }

    /// Opens System Settings to the most relevant privacy page.
    ///
    /// # Errors
    /// Returns an error if the `open` command fails.
    fn open_system_settings(issues: &[PermissionIssue]) -> Result<()> {
        let url = settings_url_for_issues(issues);
        Command::new("open")
            .arg(url)
            .status()
            .with_context(|| format!("failed to open System Settings URL {url}"))?;
        Ok(())
    }

    /// Builds the user-facing permission summary shown at startup.
    fn build_permission_dialog_message(
        issues: &[PermissionIssue],
        launch_context: &LaunchContext,
    ) -> String {
        let mut lines = vec![String::from(
            "WhisperInput started without all required permissions.",
        )];
        lines.push(String::from(""));
        lines.push(format!(
            "Permission target: {}",
            launch_context.permission_target_path().display()
        ));

        if let Some(expected_login_app_path) = &launch_context.expected_login_app_path {
            lines.push(format!(
                "Expected login app: {}",
                expected_login_app_path.display()
            ));

            if launch_context.login_target_matches_current_runtime() == Some(false) {
                lines.push(String::from(
                    "The current runtime does not match the installed login app. Permissions granted to one path will not apply to the other.",
                ));
            }
        }

        lines.push(String::from(""));

        for issue in issues {
            lines.push(match issue {
                PermissionIssue::Accessibility => {
                    String::from("- Accessibility: needed to paste transcript text into the focused app.")
                }
                PermissionIssue::InputMonitoring => {
                    String::from("- Input Monitoring: needed to detect the global hotkey while running in the background.")
                }
                PermissionIssue::Microphone => {
                    String::from("- Microphone: needed to capture voice input.")
                }
            });
        }

        lines.push(String::from(""));
        lines.push(String::from(
            "WhisperInput already asked macOS for any permission that supports a direct prompt.",
        ));
        lines.push(String::from(
            "Open System Settings now for anything that still needs manual approval. After changing permissions, quit and reopen WhisperInput if a feature stays unavailable.",
        ));
        lines.join("\n")
    }

    /// Builds the user-facing permission status summary used by tray diagnostics.
    fn build_permission_status_dialog_message(report: &PermissionReport) -> String {
        let mut lines = vec![String::from(
            "WhisperInput permission status for this running app.",
        )];
        lines.push(String::from(""));
        lines.push(format!(
            "Permission target: {}",
            report.launch_context.permission_target_path().display()
        ));

        if let Some(expected_login_app_path) = &report.launch_context.expected_login_app_path {
            lines.push(format!(
                "Expected login app: {}",
                expected_login_app_path.display()
            ));

            if report.launch_context.login_target_matches_current_runtime() == Some(false) {
                lines.push(String::from(
                    "The current runtime does not match the installed login app. Permissions granted to one path will not apply to the other.",
                ));
            }
        }

        lines.push(String::from(""));
        lines.push(format!(
            "- Input Monitoring / Hotkey: {}",
            permission_status_label(report.input_monitoring_granted)
        ));
        lines.push(format!(
            "- Accessibility / Paste: {}",
            permission_status_label(report.accessibility_granted)
        ));
        lines.push(format!(
            "- Microphone / Recording: {}",
            permission_status_label(report.microphone_granted)
        ));
        lines.push(String::from(""));

        if report.issues.is_empty() {
            lines.push(String::from(
                "All required permissions are currently granted.",
            ));
        } else {
            lines.push(String::from(
                "Missing permissions can prevent the global hotkey, recording, or auto-paste from working.",
            ));
            lines.push(String::from(
                "Use Open Settings to grant access for the permission target shown above, then restart WhisperInput if a feature still does not respond.",
            ));
        }

        lines.join("\n")
    }

    /// Formats a boolean permission state for status-dialog display.
    fn permission_status_label(granted: bool) -> &'static str {
        if granted { "Granted" } else { "Missing" }
    }

    /// Chooses the best System Settings deep link for the current issue set.
    fn settings_url_for_issues(issues: &[PermissionIssue]) -> &'static str {
        match issues {
            [PermissionIssue::Accessibility] => SETTINGS_ACCESSIBILITY_URL,
            [PermissionIssue::InputMonitoring] => SETTINGS_INPUT_MONITORING_URL,
            [PermissionIssue::Microphone] => SETTINGS_MICROPHONE_URL,
            _ => SETTINGS_PRIVACY_GENERAL_URL,
        }
    }

    /// Extracts the enclosing `.app` bundle path from an executable path.
    fn app_bundle_path_for_executable(executable_path: &Path) -> Option<PathBuf> {
        executable_path.ancestors().find_map(|ancestor| {
            let extension = ancestor.extension()?;
            if extension == "app" {
                Some(ancestor.to_path_buf())
            } else {
                None
            }
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{
            LaunchContext, MicrophoneAuthorizationStatus, PermissionIssue, PermissionReport,
            app_bundle_path_for_executable, build_permission_dialog_message,
            build_permission_status_dialog_message, permission_issues_from_grants,
            settings_url_for_issues,
        };
        use std::path::{Path, PathBuf};

        fn sample_launch_context() -> LaunchContext {
            LaunchContext {
                current_executable_path: PathBuf::from(
                    "/Users/grad/Applications/WhisperInput.app/Contents/MacOS/whisper_input",
                ),
                current_app_bundle_path: Some(PathBuf::from(
                    "/Users/grad/Applications/WhisperInput.app",
                )),
                expected_login_app_path: Some(PathBuf::from(
                    "/Users/grad/Applications/WhisperInput.app",
                )),
            }
        }

        fn sample_permission_report() -> PermissionReport {
            PermissionReport {
                launch_context: sample_launch_context(),
                accessibility_granted: true,
                input_monitoring_granted: false,
                microphone_granted: true,
                issues: vec![PermissionIssue::InputMonitoring],
            }
        }

        #[test]
        fn microphone_status_maps_known_values() {
            assert_eq!(
                MicrophoneAuthorizationStatus::from_raw(0),
                MicrophoneAuthorizationStatus::NotDetermined
            );
            assert_eq!(
                MicrophoneAuthorizationStatus::from_raw(1),
                MicrophoneAuthorizationStatus::Restricted
            );
            assert_eq!(
                MicrophoneAuthorizationStatus::from_raw(2),
                MicrophoneAuthorizationStatus::Denied
            );
            assert_eq!(
                MicrophoneAuthorizationStatus::from_raw(3),
                MicrophoneAuthorizationStatus::Authorized
            );
            assert_eq!(
                MicrophoneAuthorizationStatus::from_raw(7),
                MicrophoneAuthorizationStatus::Unknown(7)
            );
        }

        #[test]
        fn permission_dialog_lists_requested_permissions() {
            let launch_context = sample_launch_context();
            let message = build_permission_dialog_message(
                &[
                    PermissionIssue::Accessibility,
                    PermissionIssue::InputMonitoring,
                    PermissionIssue::Microphone,
                ],
                &launch_context,
            );
            assert!(message.contains("Accessibility"));
            assert!(message.contains("Input Monitoring"));
            assert!(message.contains("Microphone"));
            assert!(message.contains("Permission target"));
            assert!(message.contains("already asked macOS"));
        }

        #[test]
        fn permission_dialog_reports_login_target_mismatch() {
            let mut launch_context = sample_launch_context();
            launch_context.current_app_bundle_path =
                Some(PathBuf::from("/Applications/OldWhisperInput.app"));

            let message = build_permission_dialog_message(
                &[PermissionIssue::InputMonitoring],
                &launch_context,
            );

            assert!(message.contains("/Applications/OldWhisperInput.app"));
            assert!(message.contains("/Users/grad/Applications/WhisperInput.app"));
            assert!(message.contains("does not match the installed login app"));
        }

        #[test]
        fn permission_status_dialog_lists_current_states() {
            let message = build_permission_status_dialog_message(&sample_permission_report());

            assert!(message.contains("Input Monitoring / Hotkey: Missing"));
            assert!(message.contains("Accessibility / Paste: Granted"));
            assert!(message.contains("Microphone / Recording: Granted"));
            assert!(message.contains("Open Settings"));
        }

        #[test]
        fn permission_status_dialog_reports_all_clear() {
            let mut report = sample_permission_report();
            report.input_monitoring_granted = true;
            report.issues.clear();

            let message = build_permission_status_dialog_message(&report);

            assert!(message.contains("All required permissions are currently granted."));
            assert!(!message.contains("Open Settings"));
        }

        #[test]
        fn bundle_path_is_extracted_from_executable_path() {
            let bundle_path = app_bundle_path_for_executable(Path::new(
                "/Users/grad/Applications/WhisperInput.app/Contents/MacOS/whisper_input",
            ));
            assert_eq!(
                bundle_path,
                Some(PathBuf::from("/Users/grad/Applications/WhisperInput.app"))
            );
        }

        #[test]
        fn settings_url_prefers_specific_page_for_single_issue() {
            assert_eq!(
                settings_url_for_issues(&[PermissionIssue::Accessibility]),
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            );
            assert_eq!(
                settings_url_for_issues(&[PermissionIssue::InputMonitoring]),
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            );
            assert_eq!(
                settings_url_for_issues(&[PermissionIssue::Microphone]),
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            );
        }

        #[test]
        fn settings_url_uses_general_privacy_page_for_multiple_issues() {
            assert_eq!(
                settings_url_for_issues(&[
                    PermissionIssue::Accessibility,
                    PermissionIssue::InputMonitoring,
                ]),
                "x-apple.systempreferences:com.apple.preference.security"
            );
        }

        #[test]
        fn permission_issue_mapping_keeps_accessibility_missing_until_granted() {
            assert_eq!(
                permission_issues_from_grants(false, true, true),
                vec![PermissionIssue::Accessibility]
            );
            assert_eq!(
                permission_issues_from_grants(true, false, true),
                vec![PermissionIssue::InputMonitoring]
            );
            assert_eq!(
                permission_issues_from_grants(true, true, false),
                vec![PermissionIssue::Microphone]
            );
            assert_eq!(
                permission_issues_from_grants(false, false, true),
                vec![
                    PermissionIssue::Accessibility,
                    PermissionIssue::InputMonitoring
                ]
            );
        }
    }
}

/// Interprets the platform-specific lock result into a Rust `Result`.
///
/// # Errors
/// Returns an error when the file-lock syscall fails unexpectedly.
fn try_lock_file_result(result: i32) -> Result<bool> {
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(false);
    }

    Err(err).context("failed to acquire single-instance lock")
}

#[cfg(target_os = "macos")]
use macos::{run_macos_permission_diagnostics_dialog, run_macos_startup_checks, try_lock_file};

#[cfg(not(target_os = "macos"))]
fn try_lock_file(_file: &File) -> Result<bool> {
    Ok(true)
}
