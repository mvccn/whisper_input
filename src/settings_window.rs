//! Native settings window for runtime hotkey and model-size changes.

use std::sync::Arc;

use anyhow::{Context, Result};
use handy_keys::Hotkey;
use tao::dpi::LogicalSize;
use tao::event::WindowEvent;
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder, WindowId};

use crate::hotkey::{HotkeyBinding, describe_hotkey_binding};
use crate::model::ModelSize;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSPopUpButton, NSTextAlignment, NSTextField, NSView,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
#[cfg(target_os = "macos")]
use tao::platform::macos::WindowExtMacOS;

const SETTINGS_WINDOW_TITLE: &str = "WhisperInput Settings";
const SETTINGS_WINDOW_WIDTH: f64 = 560.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 278.0;
const CAPTURE_HINT_TEXT: &str = "Press the shortcut now. Press Escape to cancel capture.";
const MODEL_NOTE_TEXT: &str =
    "Larger models are slower and may download the first time you pick them.";

/// Handles settings-window actions emitted by native controls.
pub(crate) type SettingsActionHandler = Arc<dyn Fn(SettingsAction) + Send + Sync>;

/// User intent emitted from the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsAction {
    /// Restore the default right-command-tap binding.
    UseDefaultHotkey,
    /// Begin one-shot custom hotkey capture.
    CaptureHotkey,
    /// Switch the active Whisper model preset.
    SetModelSize(ModelSize),
}

/// Short-lived capture state shown inside the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureStage {
    Idle,
    Capturing,
}

/// Stateful data mirrored into the native controls.
#[derive(Debug, Clone)]
struct SettingsState {
    hotkey_binding: HotkeyBinding,
    model_size: ModelSize,
    capture_stage: CaptureStage,
    status_text: Option<String>,
}

impl SettingsState {
    /// Builds the initial window state from the app configuration.
    fn new(initial_model_size: ModelSize, initial_hotkey_binding: &HotkeyBinding) -> Self {
        Self {
            hotkey_binding: initial_hotkey_binding.clone(),
            model_size: initial_model_size,
            capture_stage: CaptureStage::Idle,
            status_text: None,
        }
    }

    /// Formats the active hotkey for UI display.
    fn hotkey_label(&self) -> String {
        if matches!(self.capture_stage, CaptureStage::Capturing) {
            return String::from("Listening...");
        }

        describe_hotkey_binding(&self.hotkey_binding)
    }

    /// Returns the transient inline status text, if any.
    fn status_label(&self) -> Option<&str> {
        self.status_text.as_deref()
    }
}

/// Native settings window controller.
pub(crate) struct SettingsWindow {
    state: SettingsState,
    inner: Option<SettingsWindowInner>,
}

impl SettingsWindow {
    /// Creates the controller with the initial app state.
    pub(crate) fn new(
        initial_model_size: ModelSize,
        initial_hotkey_binding: &HotkeyBinding,
    ) -> Self {
        Self {
            state: SettingsState::new(initial_model_size, initial_hotkey_binding),
            inner: None,
        }
    }

    /// Opens the settings window or focuses the existing instance.
    ///
    /// # Errors
    /// Returns an error when tao or native macOS control setup fails.
    pub(crate) fn show<T: 'static>(
        &mut self,
        event_loop: &EventLoopWindowTarget<T>,
        action_handler: SettingsActionHandler,
    ) -> Result<()> {
        if let Some(inner) = self.inner.as_ref() {
            inner.window.set_visible(true);
            inner.window.set_focus();
            return self.sync();
        }

        let window = WindowBuilder::new()
            .with_title(SETTINGS_WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(
                SETTINGS_WINDOW_WIDTH,
                SETTINGS_WINDOW_HEIGHT,
            ))
            .with_resizable(false)
            .with_visible(false)
            .build(event_loop)
            .context("failed to create settings window")?;

        self.inner = Some(build_settings_window_inner(window, action_handler)?);
        self.sync()?;

        if let Some(inner) = self.inner.as_ref() {
            inner.window.set_visible(true);
            inner.window.set_focus();
        }

        Ok(())
    }

    /// Hides the settings window when its close button is pressed.
    pub(crate) fn handle_window_event(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let Some(inner) = self.inner.as_ref() else {
            return false;
        };
        if inner.window.id() != window_id {
            return false;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            inner.window.set_visible(false);
        }
        true
    }

    /// Updates the hotkey preview after a successful binding change.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn set_hotkey_binding(&mut self, binding: HotkeyBinding) -> Result<()> {
        self.state.hotkey_binding = binding;
        self.state.capture_stage = CaptureStage::Idle;
        self.state.status_text = None;
        self.sync()
    }

    /// Shows the in-window capture hint while the listener waits for input.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn begin_hotkey_capture(&mut self) -> Result<()> {
        self.state.capture_stage = CaptureStage::Capturing;
        self.state.status_text = Some(String::from(CAPTURE_HINT_TEXT));
        self.sync()
    }

    /// Applies the captured shortcut to the active hotkey row.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn show_captured_hotkey(&mut self, hotkey: Hotkey) -> Result<()> {
        self.state.hotkey_binding = HotkeyBinding::KeyCombo(hotkey);
        self.state.capture_stage = CaptureStage::Idle;
        self.state.status_text = None;
        self.sync()
    }

    /// Restores the idle state after capture is cancelled by Escape.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn cancel_hotkey_capture(&mut self) -> Result<()> {
        self.state.capture_stage = CaptureStage::Idle;
        self.state.status_text = None;
        self.sync()
    }

    /// Updates the selected Whisper model in the window.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn set_model_size(&mut self, model_size: ModelSize) -> Result<()> {
        self.state.model_size = model_size;
        if matches!(self.state.capture_stage, CaptureStage::Idle) {
            self.state.status_text = None;
        }
        self.sync()
    }

    /// Shows an error message from the runtime or settings workflow.
    ///
    /// # Errors
    /// Returns an error when the native settings controls cannot be refreshed.
    pub(crate) fn show_error(&mut self, message: &str) -> Result<()> {
        self.state.status_text = Some(format!("Error: {message}"));
        self.sync()
    }

    /// Pushes the current state into the active settings controls, if the window exists.
    ///
    /// # Errors
    /// Returns an error when the native settings controls reject a refresh.
    fn sync(&self) -> Result<()> {
        let Some(inner) = self.inner.as_ref() else {
            return Ok(());
        };
        inner.sync(&self.state)
    }
}

/// Retained native settings window state.
struct SettingsWindowInner {
    window: Window,
    #[cfg(target_os = "macos")]
    controls: NativeSettingsControls,
}

impl SettingsWindowInner {
    /// Applies the latest settings state to the platform-native controls.
    ///
    /// # Errors
    /// Returns an error when the active platform-specific controls cannot be updated.
    fn sync(&self, state: &SettingsState) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.controls.apply_state(state)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = state;
            Ok(())
        }
    }
}

/// Builds the platform-native settings window contents.
///
/// # Errors
/// Returns an error when native control construction fails.
fn build_settings_window_inner(
    window: Window,
    action_handler: SettingsActionHandler,
) -> Result<SettingsWindowInner> {
    #[cfg(target_os = "macos")]
    {
        let controls = NativeSettingsControls::attach(&window, action_handler)?;
        Ok(SettingsWindowInner { window, controls })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = action_handler;
        Ok(SettingsWindowInner { window })
    }
}

/// Returns the display title used for a model-size option.
fn model_size_title(model_size: ModelSize) -> &'static str {
    match model_size {
        ModelSize::Tiny => "Tiny",
        ModelSize::Base => "Base",
        ModelSize::Small => "Small",
        ModelSize::Medium => "Medium",
        ModelSize::Large => "Large",
    }
}

/// Returns the popup index used for a model-size option.
fn popup_index_for_model_size(model_size: ModelSize) -> isize {
    match model_size {
        ModelSize::Tiny => 0,
        ModelSize::Base => 1,
        ModelSize::Small => 2,
        ModelSize::Medium => 3,
        ModelSize::Large => 4,
    }
}

/// Parses a popup selection index back into a model-size preset.
fn model_size_from_popup_index(index: isize) -> Option<ModelSize> {
    match index {
        0 => Some(ModelSize::Tiny),
        1 => Some(ModelSize::Base),
        2 => Some(ModelSize::Small),
        3 => Some(ModelSize::Medium),
        4 => Some(ModelSize::Large),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct NativeActionTargetIvars {
    action_handler: SettingsActionHandler,
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = NativeActionTargetIvars]
    struct NativeActionTarget;

    unsafe impl NSObjectProtocol for NativeActionTarget {}

    impl NativeActionTarget {
        #[unsafe(method(useDefaultHotkey:))]
        fn use_default_hotkey(&self, _sender: &NSButton) {
            self.post(SettingsAction::UseDefaultHotkey);
        }

        #[unsafe(method(captureHotkey:))]
        fn capture_hotkey(&self, _sender: &NSButton) {
            self.post(SettingsAction::CaptureHotkey);
        }

        #[unsafe(method(modelSizeChanged:))]
        fn model_size_changed(&self, sender: &NSPopUpButton) {
            if let Some(model_size) = model_size_from_popup_index(sender.indexOfSelectedItem()) {
                self.post(SettingsAction::SetModelSize(model_size));
            }
        }
    }
);

#[cfg(target_os = "macos")]
impl NativeActionTarget {
    /// Creates the Objective-C target object used by native controls.
    fn new(mtm: MainThreadMarker, action_handler: SettingsActionHandler) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NativeActionTargetIvars { action_handler });
        unsafe { msg_send![super(this), init] }
    }

    /// Forwards a control action into the Rust settings event handler.
    fn post(&self, action: SettingsAction) {
        (self.ivars().action_handler)(action);
    }
}

#[cfg(target_os = "macos")]
struct NativeSettingsControls {
    _root_view: Retained<NSView>,
    _action_target: Retained<NativeActionTarget>,
    hotkey_value: Retained<NSTextField>,
    status_label: Retained<NSTextField>,
    use_default_button: Retained<NSButton>,
    capture_button: Retained<NSButton>,
    model_popup: Retained<NSPopUpButton>,
}

#[cfg(target_os = "macos")]
impl NativeSettingsControls {
    /// Attaches AppKit controls to the tao-owned native window.
    ///
    /// # Errors
    /// Returns an error when the native content view cannot be resolved on the main thread.
    fn attach(window: &Window, action_handler: SettingsActionHandler) -> Result<Self> {
        let mtm =
            MainThreadMarker::new().context("settings window must be built on the main thread")?;
        let root_view = unsafe { Retained::retain(window.ns_view().cast::<NSView>()) }
            .context("failed to retain settings content view")?;
        let action_target = NativeActionTarget::new(mtm, action_handler);

        let hotkey_label = make_row_label(mtm, "Hotkey:", rect(42.0, 210.0, 92.0, 17.0));
        add_subview(&root_view, &hotkey_label);

        let hotkey_value = make_readonly_field(mtm, "", rect(148.0, 204.0, 148.0, 24.0));
        add_subview(&root_view, &hotkey_value);

        let use_default_button = make_button(
            mtm,
            "Reset Default",
            rect(148.0, 166.0, 96.0, 28.0),
            &action_target,
            sel!(useDefaultHotkey:),
        );
        add_subview(&root_view, &use_default_button);

        let capture_button = make_button(
            mtm,
            "Capture New Combo",
            rect(308.0, 203.0, 132.0, 28.0),
            &action_target,
            sel!(captureHotkey:),
        );
        add_subview(&root_view, &capture_button);

        let status_label = make_wrapping_label(mtm, "", rect(148.0, 128.0, 400.0, 28.0));
        status_label.setHidden(true);
        add_subview(&root_view, &status_label);

        let middle_separator = make_separator(mtm, rect(20.0, 102.0, 520.0, 2.0));
        add_subview(&root_view, &middle_separator);

        let model_label = make_row_label(mtm, "Model:", rect(42.0, 62.0, 92.0, 17.0));
        add_subview(&root_view, &model_label);

        let model_popup = make_model_popup(mtm, &action_target, rect(148.0, 56.0, 150.0, 26.0));
        add_subview(&root_view, &model_popup);

        let model_note = make_wrapping_label(mtm, MODEL_NOTE_TEXT, rect(148.0, 18.0, 360.0, 30.0));
        add_subview(&root_view, &model_note);

        Ok(Self {
            _root_view: root_view,
            _action_target: action_target,
            hotkey_value,
            status_label,
            use_default_button,
            capture_button,
            model_popup,
        })
    }

    /// Applies the current Rust state to the native AppKit controls.
    ///
    /// # Errors
    /// Returns an error when the active model selection cannot be represented by the popup.
    fn apply_state(&self, state: &SettingsState) -> Result<()> {
        self.hotkey_value
            .setStringValue(&NSString::from_str(&state.hotkey_label()));

        if let Some(status_text) = state.status_label() {
            self.status_label
                .setStringValue(&NSString::from_str(status_text));
            self.status_label.setHidden(false);
        } else {
            self.status_label.setHidden(true);
        }

        let selected_index = popup_index_for_model_size(state.model_size);
        self.model_popup.selectItemAtIndex(selected_index);
        if self.model_popup.indexOfSelectedItem() != selected_index {
            return Err(anyhow::anyhow!(
                "settings model popup could not select index {selected_index}"
            ));
        }

        match state.capture_stage {
            CaptureStage::Idle => {
                self.use_default_button.setEnabled(true);
                self.capture_button.setEnabled(true);
                self.capture_button
                    .setTitle(&NSString::from_str("Capture New Combo"));
            }
            CaptureStage::Capturing => {
                self.use_default_button.setEnabled(false);
                self.capture_button.setEnabled(false);
                self.capture_button
                    .setTitle(&NSString::from_str("Listening..."));
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

#[cfg(target_os = "macos")]
fn add_subview(parent: &NSView, child: &NSView) {
    parent.addSubview(child);
}

#[cfg(target_os = "macos")]
fn make_label(mtm: MainThreadMarker, value: &str, frame: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(value), mtm);
    label.setFrame(frame);
    label
}

#[cfg(target_os = "macos")]
fn make_row_label(mtm: MainThreadMarker, value: &str, frame: NSRect) -> Retained<NSTextField> {
    let label = make_label(mtm, value, frame);
    label.setAlignment(NSTextAlignment::Right);
    label
}

#[cfg(target_os = "macos")]
fn make_wrapping_label(mtm: MainThreadMarker, value: &str, frame: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(value), mtm);
    label.setFrame(frame);
    label.setSelectable(false);
    label
}

#[cfg(target_os = "macos")]
fn make_readonly_field(mtm: MainThreadMarker, value: &str, frame: NSRect) -> Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&NSString::from_str(value), mtm);
    field.setFrame(frame);
    field.setEditable(false);
    field.setSelectable(true);
    field.setBezeled(true);
    field.setBordered(true);
    field.setDrawsBackground(true);
    field
}

#[cfg(target_os = "macos")]
fn make_separator(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSBox> {
    let separator = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    separator.setTitle(&NSString::from_str(""));
    separator.setBoxType(NSBoxType::Separator);
    separator
}

#[cfg(target_os = "macos")]
fn make_button(
    mtm: MainThreadMarker,
    title: &str,
    frame: NSRect,
    action_target: &NativeActionTarget,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(action_target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button
}

#[cfg(target_os = "macos")]
fn make_model_popup(
    mtm: MainThreadMarker,
    action_target: &NativeActionTarget,
    frame: NSRect,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), frame, false);
    popup.addItemWithTitle(&NSString::from_str(model_size_title(ModelSize::Tiny)));
    popup.addItemWithTitle(&NSString::from_str(model_size_title(ModelSize::Base)));
    popup.addItemWithTitle(&NSString::from_str(model_size_title(ModelSize::Small)));
    popup.addItemWithTitle(&NSString::from_str(model_size_title(ModelSize::Medium)));
    popup.addItemWithTitle(&NSString::from_str(model_size_title(ModelSize::Large)));
    unsafe {
        popup.setTarget(Some(action_target));
        popup.setAction(Some(sel!(modelSizeChanged:)));
    }
    popup
}

#[cfg(test)]
mod tests {
    use handy_keys::{Hotkey, Key, Modifiers};

    use super::{
        CAPTURE_HINT_TEXT, CaptureStage, SettingsWindow, model_size_from_popup_index,
        popup_index_for_model_size,
    };
    use crate::hotkey::{CommandKeySide, HotkeyBinding};
    use crate::model::ModelSize;

    #[test]
    fn model_size_popup_mapping_round_trips() {
        for model_size in [
            ModelSize::Tiny,
            ModelSize::Base,
            ModelSize::Small,
            ModelSize::Medium,
            ModelSize::Large,
        ] {
            let index = popup_index_for_model_size(model_size);
            assert_eq!(model_size_from_popup_index(index), Some(model_size));
        }
        assert_eq!(model_size_from_popup_index(99), None);
    }

    #[test]
    fn captured_hotkey_is_applied_immediately_to_state() {
        let mut settings_window = SettingsWindow::new(
            ModelSize::Base,
            &HotkeyBinding::CommandTap(CommandKeySide::Right),
        );
        let captured =
            Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::Space).expect("valid hotkey");

        settings_window
            .show_captured_hotkey(captured)
            .expect("preview update should succeed without an open window");

        assert_eq!(
            settings_window.state.hotkey_binding,
            HotkeyBinding::KeyCombo(captured)
        );
        assert_eq!(settings_window.state.capture_stage, CaptureStage::Idle);
        assert_eq!(settings_window.state.status_text, None);
    }

    #[test]
    fn begin_hotkey_capture_shows_inline_status() {
        let mut settings_window = SettingsWindow::new(
            ModelSize::Base,
            &HotkeyBinding::CommandTap(CommandKeySide::Right),
        );

        settings_window
            .begin_hotkey_capture()
            .expect("capture state update should succeed");

        assert_eq!(settings_window.state.capture_stage, CaptureStage::Capturing);
        assert_eq!(
            settings_window.state.status_text.as_deref(),
            Some(CAPTURE_HINT_TEXT)
        );
    }
}
