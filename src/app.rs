//! Runtime orchestration for tray UI, hotkey control, audio capture, Whisper, and output.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use handy_keys::Hotkey;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tracing::{error, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::audio::{ActiveRecording, Recorder, has_minimum_signal, to_whisper_samples};
use crate::config::Config;
use crate::hotkey::{self, HotkeyBinding, HotkeyControl, HotkeyEvent, describe_hotkey_binding};
use crate::model::{self, ModelSize};
use crate::paste;
use crate::settings_window::{SettingsAction, SettingsActionHandler, SettingsWindow};
use crate::sound::{self, SoundCue};
use crate::startup;
use crate::transcribe::WhisperEngine;

const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(180);
const MENU_ID_TOGGLE: &str = "toggle";
const MENU_ID_SETTINGS: &str = "settings";
const MENU_ID_DIAGNOSE_PERMISSIONS: &str = "diagnose_permissions";
const MENU_ID_QUIT: &str = "quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    Recording,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToggleAction {
    Start,
    StopAndProcess,
}

/// UI-visible state for the tray icon and menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppStatus {
    Initializing,
    Idle,
    Listening,
    Processing,
    Error,
}

/// Commands sent from UI/hotkey to the worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkerCommand {
    ToggleRecording,
    BeginHotkeyCapture,
    SetHotkeyBinding(HotkeyBinding),
    SetModelSize(ModelSize),
    Quit,
}

/// Notifications sent from the worker back to the tray UI.
#[derive(Debug, Clone)]
enum WorkerEvent {
    Status(AppStatus),
    Error(String),
    HotkeyCaptureStarted,
    HotkeyCaptureCompleted(Hotkey),
    HotkeyCaptureCancelled,
    HotkeyBindingChanged(HotkeyBinding),
    ModelSizeChanged(ModelSize),
    PasteRequested,
    TranscriptReady(usize),
}

/// Custom tao event payload for worker and menu notifications.
#[derive(Debug, Clone)]
enum UserEvent {
    Worker(WorkerEvent),
    Menu(MenuEvent),
    Settings(SettingsAction),
}

/// Runs the application as a menu bar utility.
///
/// # Errors
/// Returns an error when tray UI setup fails before the event loop starts.
pub(crate) fn run(config: Config) -> Result<()> {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    #[cfg(target_os = "macos")]
    {
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop.set_dock_visibility(false);
    }

    let proxy = event_loop.create_proxy();
    register_menu_event_proxy(proxy.clone());

    let initial_model_size = config.model_size;
    let initial_hotkey_binding = HotkeyBinding::CommandTap(config.command_key);
    let worker_tx = spawn_worker(config, proxy.clone());
    let settings_proxy = proxy.clone();
    let settings_action_handler: SettingsActionHandler = Arc::new(move |action| {
        let _ = settings_proxy.send_event(UserEvent::Settings(action));
    });
    let mut tray_ui = TrayUi::new()?;
    let mut settings_window = SettingsWindow::new(initial_model_size, &initial_hotkey_binding);

    event_loop.run(move |event, event_loop, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + ANIMATION_FRAME_INTERVAL);

        match event {
            Event::NewEvents(StartCause::Init) => {
                info!("menu bar app started");
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Err(err) = tray_ui.advance_animation() {
                    error!(error = %err, "failed to advance tray animation");
                }
            }
            Event::UserEvent(UserEvent::Worker(worker_event)) => {
                if let Err(err) =
                    apply_worker_event(worker_event, &worker_tx, &mut tray_ui, &mut settings_window)
                {
                    error!(error = %err, "failed to update tray state");
                }
            }
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                match handle_menu_event(
                    &menu_event,
                    &worker_tx,
                    &mut settings_window,
                    event_loop,
                    settings_action_handler.clone(),
                ) {
                    Ok(true) => *control_flow = ControlFlow::Exit,
                    Ok(false) => {}
                    Err(err) => {
                        error!(error = %err, "failed to handle tray menu event");
                    }
                }
            }
            Event::UserEvent(UserEvent::Settings(settings_action)) => {
                match handle_settings_action(settings_action, &worker_tx) {
                    Ok(true) => *control_flow = ControlFlow::Exit,
                    Ok(false) => {}
                    Err(err) => {
                        error!(error = %err, "failed to handle settings window action");
                    }
                }
            }
            Event::WindowEvent {
                window_id, event, ..
            } => {
                if settings_window.handle_window_event(window_id, &event)
                    && matches!(event, WindowEvent::CloseRequested)
                {
                    info!("hid settings window");
                }
            }
            Event::LoopDestroyed => {
                let _ = worker_tx.send(WorkerCommand::Quit);
            }
            _ => {}
        }
    });
}

/// Applies worker output on the main tao event loop thread.
///
/// # Errors
/// Returns an error if tray updates or key simulation fail.
fn apply_worker_event(
    event: WorkerEvent,
    worker_tx: &Sender<WorkerCommand>,
    tray_ui: &mut TrayUi,
    settings_window: &mut SettingsWindow,
) -> Result<()> {
    if let WorkerEvent::HotkeyCaptureCompleted(captured_hotkey) = &event
        && worker_tx
            .send(WorkerCommand::SetHotkeyBinding(HotkeyBinding::KeyCombo(
                *captured_hotkey,
            )))
            .is_err()
    {
        warn!("worker command channel closed while auto-applying captured hotkey");
    }

    sync_settings_window(settings_window, &event)?;

    match event {
        WorkerEvent::PasteRequested => {
            paste::paste_cmd_v().context("failed to simulate cmd+v on main event loop thread")?;
        }
        other => {
            tray_ui.handle_worker_event(other)?;
        }
    }

    Ok(())
}

/// Forwards tray menu callbacks into the tao event loop.
fn register_menu_event_proxy(proxy: EventLoopProxy<UserEvent>) {
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));
}

/// Handles tray menu interactions and returns true when app exit is requested.
fn handle_menu_event(
    event: &MenuEvent,
    worker_tx: &Sender<WorkerCommand>,
    settings_window: &mut SettingsWindow,
    event_loop: &EventLoopWindowTarget<UserEvent>,
    settings_action_handler: SettingsActionHandler,
) -> Result<bool> {
    if event.id == MENU_ID_SETTINGS {
        settings_window
            .show(event_loop, settings_action_handler)
            .context("failed to open settings window")?;
        return Ok(false);
    }

    if event.id == MENU_ID_DIAGNOSE_PERMISSIONS {
        startup::show_permission_diagnostics_dialog()
            .context("failed to show permission diagnostics dialog")?;
        return Ok(false);
    }

    if event.id == MENU_ID_TOGGLE {
        if worker_tx.send(WorkerCommand::ToggleRecording).is_err() {
            warn!("worker command channel closed while toggling recording");
            return Ok(true);
        }
        return Ok(false);
    }

    if event.id == MENU_ID_QUIT {
        let _ = worker_tx.send(WorkerCommand::Quit);
        return Ok(true);
    }

    Ok(false)
}

/// Applies worker-driven state changes to the settings window.
///
/// # Errors
/// Returns an error if the native settings window cannot be updated.
fn sync_settings_window(settings_window: &mut SettingsWindow, event: &WorkerEvent) -> Result<()> {
    match event {
        WorkerEvent::Error(message) => settings_window.show_error(message),
        WorkerEvent::HotkeyCaptureStarted => settings_window.begin_hotkey_capture(),
        WorkerEvent::HotkeyCaptureCompleted(captured_hotkey) => {
            settings_window.show_captured_hotkey(*captured_hotkey)
        }
        WorkerEvent::HotkeyCaptureCancelled => settings_window.cancel_hotkey_capture(),
        WorkerEvent::HotkeyBindingChanged(binding) => {
            settings_window.set_hotkey_binding(binding.clone())
        }
        WorkerEvent::ModelSizeChanged(model_size) => settings_window.set_model_size(*model_size),
        WorkerEvent::Status(_) | WorkerEvent::PasteRequested | WorkerEvent::TranscriptReady(_) => {
            Ok(())
        }
    }
}

/// Handles actions posted from the native settings window.
///
/// # Errors
/// Returns an error if a settings action cannot be forwarded to the worker.
fn handle_settings_action(
    action: SettingsAction,
    worker_tx: &Sender<WorkerCommand>,
) -> Result<bool> {
    match action {
        SettingsAction::UseDefaultHotkey => {
            if worker_tx
                .send(WorkerCommand::SetHotkeyBinding(
                    HotkeyBinding::default_command_tap(),
                ))
                .is_err()
            {
                warn!("worker command channel closed while setting default hotkey");
                return Ok(true);
            }
        }
        SettingsAction::CaptureHotkey => {
            if worker_tx.send(WorkerCommand::BeginHotkeyCapture).is_err() {
                warn!("worker command channel closed while starting hotkey capture");
                return Ok(true);
            }
        }
        SettingsAction::SetModelSize(model_size) => {
            if worker_tx
                .send(WorkerCommand::SetModelSize(model_size))
                .is_err()
            {
                warn!("worker command channel closed while changing model size");
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Spawns the background worker thread and returns a command sender.
fn spawn_worker(config: Config, proxy: EventLoopProxy<UserEvent>) -> Sender<WorkerCommand> {
    let (command_tx, command_rx) = mpsc::channel();

    thread::spawn(move || {
        if let Err(err) = worker_main(config, command_rx, proxy) {
            error!(error = %err, "worker terminated with error");
        }
    });

    command_tx
}

/// Runs the core recording/transcription loop in a background worker.
///
/// # Errors
/// Returns an error if hotkey listener initialization fails critically.
fn worker_main(
    config: Config,
    command_rx: Receiver<WorkerCommand>,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<()> {
    let mut config = config;
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let initial_hotkey_binding = HotkeyBinding::CommandTap(config.command_key);
    let hotkey_control = hotkey::spawn_listener(
        hotkey_tx,
        initial_hotkey_binding.clone(),
        Duration::from_millis(config.hotkey_max_tap_ms),
    );
    send_worker_event(
        &proxy,
        WorkerEvent::HotkeyBindingChanged(initial_hotkey_binding),
    );

    let mut runtime = initialize_runtime(&config, &proxy);

    loop {
        while let Ok(hotkey_event) = hotkey_rx.try_recv() {
            match hotkey_event {
                HotkeyEvent::ToggleRecording => {
                    handle_toggle_request(&mut runtime, &config, &proxy);
                }
                HotkeyEvent::CapturedKeyCombo(captured_hotkey) => {
                    handle_hotkey_combo_captured(captured_hotkey, &proxy);
                }
                HotkeyEvent::CaptureCancelled => {
                    send_worker_event(&proxy, WorkerEvent::HotkeyCaptureCancelled);
                }
            }
        }

        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(WorkerCommand::ToggleRecording) => {
                handle_toggle_request(&mut runtime, &config, &proxy);
            }
            Ok(WorkerCommand::BeginHotkeyCapture) => {
                handle_begin_hotkey_capture(&hotkey_control, &proxy);
            }
            Ok(WorkerCommand::SetHotkeyBinding(binding)) => {
                handle_hotkey_binding_change(&mut config, &hotkey_control, binding, &proxy);
            }
            Ok(WorkerCommand::SetModelSize(model_size)) => {
                handle_model_size_change(&mut runtime, &mut config, model_size, &proxy);
            }
            Ok(WorkerCommand::Quit) => {
                info!("worker received quit signal");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                warn!("worker command channel disconnected");
                break;
            }
        }
    }

    Ok(())
}

/// Applies a requested hotkey binding change for the listener thread.
fn handle_hotkey_binding_change(
    config: &mut Config,
    hotkey_control: &HotkeyControl,
    requested_binding: HotkeyBinding,
    proxy: &EventLoopProxy<UserEvent>,
) {
    if hotkey_control.current_binding() == requested_binding {
        send_worker_event(proxy, WorkerEvent::HotkeyBindingChanged(requested_binding));
        return;
    }

    if let HotkeyBinding::CommandTap(command_key_side) = requested_binding.clone() {
        config.command_key = command_key_side;
    }

    hotkey_control.set_binding(requested_binding.clone());
    info!(
        hotkey_binding = %describe_hotkey_binding(&requested_binding),
        "updated hotkey binding"
    );
    send_worker_event(proxy, WorkerEvent::HotkeyBindingChanged(requested_binding));
}

/// Arms one-shot capture for the next key combination pressed by the user.
fn handle_begin_hotkey_capture(hotkey_control: &HotkeyControl, proxy: &EventLoopProxy<UserEvent>) {
    hotkey_control.request_capture();
    send_worker_event(proxy, WorkerEvent::HotkeyCaptureStarted);
}

/// Forwards a newly captured key combination to the tray for confirmation.
fn handle_hotkey_combo_captured(captured_hotkey: Hotkey, proxy: &EventLoopProxy<UserEvent>) {
    send_worker_event(proxy, WorkerEvent::HotkeyCaptureCompleted(captured_hotkey));
    info!(
        hotkey = %captured_hotkey,
        "captured hotkey combination and waiting for confirmation"
    );
}

/// Applies a model-size change without interrupting active recordings.
fn handle_model_size_change(
    runtime: &mut Option<WorkerRuntime>,
    config: &mut Config,
    requested_size: ModelSize,
    proxy: &EventLoopProxy<UserEvent>,
) {
    if config.model_size == requested_size {
        send_worker_event(proxy, WorkerEvent::ModelSizeChanged(config.model_size));
        return;
    }

    if runtime
        .as_ref()
        .is_some_and(|runtime_ref| runtime_ref.state == SessionState::Recording)
    {
        send_worker_event(
            proxy,
            WorkerEvent::Error(String::from("stop recording before changing model size")),
        );
        send_worker_event(proxy, WorkerEvent::ModelSizeChanged(config.model_size));
        return;
    }

    info!(from = ?config.model_size, to = ?requested_size, "changing model size");

    let previous_size = config.model_size;
    let previous_runtime = runtime.take();
    config.model_size = requested_size;

    let reloaded_runtime = initialize_runtime(config, proxy);
    if reloaded_runtime.is_some() {
        *runtime = reloaded_runtime;
        send_worker_event(proxy, WorkerEvent::ModelSizeChanged(config.model_size));
        return;
    }

    config.model_size = previous_size;
    *runtime = previous_runtime;
    send_worker_event(proxy, WorkerEvent::ModelSizeChanged(config.model_size));
}

/// Processes one toggle command, initializing runtime lazily when needed.
fn handle_toggle_request(
    runtime: &mut Option<WorkerRuntime>,
    config: &Config,
    proxy: &EventLoopProxy<UserEvent>,
) {
    if runtime.is_none() {
        *runtime = initialize_runtime(config, proxy);
    }

    if let Some(runtime_ref) = runtime.as_mut()
        && let Err(err) = runtime_ref.handle_toggle(proxy)
    {
        send_worker_event(proxy, WorkerEvent::Error(err.to_string()));
        send_worker_event(proxy, WorkerEvent::Status(AppStatus::Error));
        runtime_ref.state = SessionState::Idle;
        runtime_ref.active_recording = None;
        send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));
    }
}

/// Initializes model/audio runtime and reports status to the tray.
fn initialize_runtime(config: &Config, proxy: &EventLoopProxy<UserEvent>) -> Option<WorkerRuntime> {
    send_worker_event(proxy, WorkerEvent::Status(AppStatus::Initializing));

    let model_path = match model::ensure_model(config.model_size, &config.model_dir) {
        Ok(path) => path,
        Err(err) => {
            send_worker_event(proxy, WorkerEvent::Error(err.to_string()));
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Error));
            return None;
        }
    };

    let recorder = match Recorder::new(config.max_record_seconds) {
        Ok(recorder) => recorder,
        Err(err) => {
            send_worker_event(proxy, WorkerEvent::Error(err.to_string()));
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Error));
            return None;
        }
    };

    let whisper = match WhisperEngine::new(
        &model_path,
        config.threads,
        config.use_gpu,
        config.flash_attn,
    ) {
        Ok(engine) => engine,
        Err(err) => {
            send_worker_event(proxy, WorkerEvent::Error(err.to_string()));
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Error));
            return None;
        }
    };

    send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));

    Some(WorkerRuntime {
        state: SessionState::Idle,
        auto_paste: config.auto_paste,
        recorder,
        whisper,
        active_recording: None,
    })
}

/// Sends a worker event to the main event loop, ignoring shutdown races.
fn send_worker_event(proxy: &EventLoopProxy<UserEvent>, event: WorkerEvent) {
    let _ = proxy.send_event(UserEvent::Worker(event));
}

/// Worker-owned recording/transcription state.
struct WorkerRuntime {
    state: SessionState,
    auto_paste: bool,
    recorder: Recorder,
    whisper: WhisperEngine,
    active_recording: Option<ActiveRecording>,
}

impl WorkerRuntime {
    /// Applies a toggle transition and executes required side effects.
    ///
    /// # Errors
    /// Returns an error if recording, transcription, or output operations fail.
    fn handle_toggle(&mut self, proxy: &EventLoopProxy<UserEvent>) -> Result<()> {
        match next_action(self.state) {
            ToggleAction::Start => self.start_recording(proxy),
            ToggleAction::StopAndProcess => self.stop_and_process(proxy),
        }
    }

    /// Starts a recording session and updates tray state.
    ///
    /// # Errors
    /// Returns an error when microphone stream startup fails.
    fn start_recording(&mut self, proxy: &EventLoopProxy<UserEvent>) -> Result<()> {
        self.active_recording = Some(self.recorder.start()?);
        self.state = SessionState::Recording;
        send_worker_event(proxy, WorkerEvent::Status(AppStatus::Listening));
        sound::play(SoundCue::ListeningStarted);
        info!("recording started");
        Ok(())
    }

    /// Stops recording, transcribes, and pastes transcript to focused app.
    ///
    /// # Errors
    /// Returns an error if transcription or output operations fail.
    fn stop_and_process(&mut self, proxy: &EventLoopProxy<UserEvent>) -> Result<()> {
        let Some(recording) = self.active_recording.take() else {
            self.state = SessionState::Idle;
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));
            return Ok(());
        };

        self.state = SessionState::Idle;
        send_worker_event(proxy, WorkerEvent::Status(AppStatus::Processing));
        sound::play(SoundCue::ListeningStopped);

        let captured = recording.stop();
        let whisper_samples = to_whisper_samples(&captured);

        if !has_minimum_signal(&whisper_samples) {
            warn!("captured audio was too short or too quiet; skipping inference");
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));
            return Ok(());
        }

        let transcript = self.whisper.transcribe(&whisper_samples)?;
        if transcript.is_empty() {
            warn!("empty transcript produced; nothing copied");
            send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));
            return Ok(());
        }

        let char_count = transcript.chars().count();
        paste::copy_to_clipboard(&transcript)?;
        if self.auto_paste {
            send_worker_event(proxy, WorkerEvent::PasteRequested);
        }
        send_worker_event(proxy, WorkerEvent::TranscriptReady(char_count));
        send_worker_event(proxy, WorkerEvent::Status(AppStatus::Idle));
        info!(char_count, "transcription copied to clipboard");

        if !self.auto_paste {
            println!("{transcript}");
        }

        Ok(())
    }
}

/// A small menu-bar UI wrapper for icon and menu item updates.
struct TrayUi {
    tray_icon: TrayIcon,
    status_item: MenuItem,
    toggle_item: MenuItem,
    icons: TrayIcons,
    status: AppStatus,
    status_text: String,
    animation_frame: usize,
}

impl TrayUi {
    /// Builds the tray menu and initial icon state.
    ///
    /// # Errors
    /// Returns an error when icon conversion or tray setup fails.
    fn new() -> Result<Self> {
        let icons = TrayIcons::build().context("failed to build tray icons")?;

        let menu = Menu::new();
        let status_item = MenuItem::new("Status: Initializing", false, None);
        let separator1 = PredefinedMenuItem::separator();
        let toggle_item = MenuItem::with_id(MENU_ID_TOGGLE, "Start Listening", true, None);
        let separator2 = PredefinedMenuItem::separator();
        let settings_item = MenuItem::with_id(MENU_ID_SETTINGS, "Settings...", true, None);
        let diagnose_permissions_item = MenuItem::with_id(
            MENU_ID_DIAGNOSE_PERMISSIONS,
            "Diagnose Permissions...",
            true,
            None,
        );
        let separator3 = PredefinedMenuItem::separator();
        let quit_item = MenuItem::with_id(MENU_ID_QUIT, "Quit", true, None);

        menu.append_items(&[
            &status_item,
            &separator1,
            &toggle_item,
            &separator2,
            &settings_item,
            &diagnose_permissions_item,
            &separator3,
            &quit_item,
        ])
        .context("failed to build tray menu items")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("whisper_input")
            .with_icon(icons.icon(AppStatus::Initializing, 0))
            .with_icon_as_template(false)
            .build()
            .context("failed to create tray icon")?;

        Ok(Self {
            tray_icon,
            status_item,
            toggle_item,
            icons,
            status: AppStatus::Initializing,
            status_text: String::from("Status: Initializing"),
            animation_frame: 0,
        })
    }

    /// Applies worker notifications to icon/menu state.
    ///
    /// # Errors
    /// Returns an error when icon updates fail.
    fn handle_worker_event(&mut self, event: WorkerEvent) -> Result<()> {
        match event {
            WorkerEvent::Status(status) => self.apply_status(status)?,
            WorkerEvent::Error(message) => {
                self.apply_status(AppStatus::Error)?;
                self.status_text = format!("Error: {message}");
                self.status_item.set_text(self.status_text.clone());
            }
            WorkerEvent::HotkeyCaptureStarted => {
                self.show_hotkey_capture_prompt();
            }
            WorkerEvent::HotkeyCaptureCompleted(_)
            | WorkerEvent::HotkeyCaptureCancelled
            | WorkerEvent::HotkeyBindingChanged(_) => {
                self.restore_status_text();
            }
            WorkerEvent::ModelSizeChanged(_) => {}
            WorkerEvent::PasteRequested => {}
            WorkerEvent::TranscriptReady(char_count) => {
                self.status_text = format!("Transcript ready ({char_count} chars)");
                self.status_item.set_text(self.status_text.clone());
            }
        }

        Ok(())
    }

    /// Advances the tray animation when the active status uses multiple frames.
    ///
    /// # Errors
    /// Returns an error when icon updates fail.
    fn advance_animation(&mut self) -> Result<()> {
        let frame_count = self.icons.frames(self.status).frame_count();
        if frame_count <= 1 {
            return Ok(());
        }

        self.animation_frame = (self.animation_frame + 1) % frame_count;
        self.apply_current_icon()
    }

    /// Shows the temporary capture prompt while the listener waits for input.
    fn show_hotkey_capture_prompt(&mut self) {
        self.status_item
            .set_text("Status: Press a hotkey combo (Escape cancels)");
    }

    /// Restores the non-transient status text after dialogs or capture prompts.
    fn restore_status_text(&mut self) {
        self.status_item.set_text(self.status_text.clone());
    }

    /// Updates the tray icon and action label for a status state.
    ///
    /// # Errors
    /// Returns an error when icon updates fail.
    fn apply_status(&mut self, status: AppStatus) -> Result<()> {
        self.status = status;
        self.animation_frame = 0;

        match status {
            AppStatus::Initializing => {
                self.apply_current_icon()?;
                self.status_text = String::from("Status: Initializing");
                self.status_item.set_text(self.status_text.clone());
                self.toggle_item.set_text("Start Listening");
            }
            AppStatus::Idle => {
                self.apply_current_icon()?;
                self.status_text = String::from("Status: Idle");
                self.status_item.set_text(self.status_text.clone());
                self.toggle_item.set_text("Start Listening");
            }
            AppStatus::Listening => {
                self.apply_current_icon()?;
                self.status_text = String::from("Status: Listening");
                self.status_item.set_text(self.status_text.clone());
                self.toggle_item.set_text("Stop Listening");
            }
            AppStatus::Processing => {
                self.apply_current_icon()?;
                self.status_text = String::from("Status: Processing");
                self.status_item.set_text(self.status_text.clone());
                self.toggle_item.set_text("Processing...");
            }
            AppStatus::Error => {
                self.apply_current_icon()?;
                self.status_text = String::from("Status: Error");
                self.status_item.set_text(self.status_text.clone());
                self.toggle_item.set_text("Retry Start Listening");
            }
        }

        Ok(())
    }

    /// Applies the current status/frame pair to the tray icon.
    ///
    /// # Errors
    /// Returns an error when icon updates fail.
    fn apply_current_icon(&mut self) -> Result<()> {
        let is_template = self.status != AppStatus::Listening;
        self.tray_icon
            .set_icon_with_as_template(
                Some(self.icons.icon(self.status, self.animation_frame)),
                is_template,
            )
            .context("failed to set tray icon")
    }
}

/// Pre-rendered waveform icons for tray status transitions.
struct TrayIcons {
    initializing: StatusIcons,
    idle: StatusIcons,
    listening: StatusIcons,
    processing: StatusIcons,
    error: StatusIcons,
}

impl TrayIcons {
    /// Builds all waveform icon frames used by the tray.
    ///
    /// # Errors
    /// Returns an error when any icon buffer cannot be converted.
    fn build() -> Result<Self> {
        Ok(Self {
            initializing: StatusIcons::build(
                &[[2, 3, 4, 3, 2], [3, 5, 7, 5, 3], [2, 4, 6, 4, 2]],
                WaveColor::black(),
            )?,
            idle: StatusIcons::build(&[[2, 4, 7, 4, 2]], WaveColor::black())?,
            listening: StatusIcons::build(&[
                [3, 7, 11, 7, 3],
                [5, 9, 13, 9, 5],
                [7, 11, 15, 11, 7],
                [5, 8, 12, 8, 5],
            ], WaveColor::red())?,
            processing: StatusIcons::build(&[
                [11, 5, 3, 3, 3],
                [4, 11, 5, 3, 3],
                [3, 4, 11, 5, 3],
                [3, 3, 4, 11, 5],
                [3, 3, 3, 5, 11],
            ], WaveColor::black())?,
            error: StatusIcons::build(&[[2, 7, 2, 7, 2]], WaveColor::black())?,
        })
    }

    /// Returns the icon set for a specific app status.
    fn frames(&self, status: AppStatus) -> &StatusIcons {
        match status {
            AppStatus::Initializing => &self.initializing,
            AppStatus::Idle => &self.idle,
            AppStatus::Listening => &self.listening,
            AppStatus::Processing => &self.processing,
            AppStatus::Error => &self.error,
        }
    }

    /// Returns the icon frame for a specific app status.
    fn icon(&self, status: AppStatus, frame_index: usize) -> Icon {
        self.frames(status).frame(frame_index)
    }
}

/// A pre-rendered animation strip for a single status.
struct StatusIcons {
    frames: Vec<Icon>,
}

impl StatusIcons {
    /// Builds all frames for a status from waveform bar-height presets.
    ///
    /// # Errors
    /// Returns an error when any icon buffer cannot be converted.
    fn build(patterns: &[[u32; 5]], color: WaveColor) -> Result<Self> {
        let mut frames = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            frames.push(build_wave_icon(*pattern, color)?);
        }

        Ok(Self { frames })
    }

    /// Returns the number of frames in this status animation.
    fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Returns a cloned frame for the requested position.
    fn frame(&self, frame_index: usize) -> Icon {
        self.frames[frame_index % self.frames.len()].clone()
    }
}

/// Creates a small monochrome waveform tray icon.
///
/// # Errors
/// Returns an error when the raw RGBA buffer is rejected by tray-icon.
fn build_wave_icon(bar_heights: [u32; 5], color: WaveColor) -> Result<Icon> {
    let rgba = render_wave_rgba(bar_heights, color);
    Icon::from_rgba(rgba, WAVE_ICON_SIZE, WAVE_ICON_SIZE).context("invalid RGBA icon buffer")
}

/// A solid RGBA color for tray waveform bars.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WaveColor {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl WaveColor {
    /// Returns the default black menu-bar waveform color.
    fn black() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }
    }

    /// Returns the red listening waveform color.
    fn red() -> Self {
        Self {
            r: 255,
            g: 59,
            b: 48,
            a: 255,
        }
    }
}

const WAVE_ICON_SIZE: u32 = 18;
const WAVE_BAR_WIDTH: u32 = 2;
const WAVE_BAR_GAP: u32 = 1;

/// Renders waveform bars into an RGBA buffer for tray icon creation.
fn render_wave_rgba(bar_heights: [u32; 5], color: WaveColor) -> Vec<u8> {
    let mut rgba = vec![0_u8; (WAVE_ICON_SIZE * WAVE_ICON_SIZE * 4) as usize];
    let total_width = (WAVE_BAR_WIDTH * bar_heights.len() as u32) + (WAVE_BAR_GAP * 4);
    let start_x = (WAVE_ICON_SIZE - total_width) / 2;

    for (index, height) in bar_heights.into_iter().enumerate() {
        let height = height.min(WAVE_ICON_SIZE.saturating_sub(4)).max(2);
        let x = start_x + index as u32 * (WAVE_BAR_WIDTH + WAVE_BAR_GAP);
        let y = (WAVE_ICON_SIZE - height) / 2;
        draw_rounded_bar(&mut rgba, WAVE_ICON_SIZE, x, y, WAVE_BAR_WIDTH, height, color);
    }

    rgba
}

/// Rasterizes a rounded vertical bar into the RGBA icon buffer.
fn draw_rounded_bar(
    rgba: &mut [u8],
    icon_size: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: WaveColor,
) {
    let radius = width as f32 / 2.0;

    for dy in 0..height {
        for dx in 0..width {
            let px = x + dx;
            let py = y + dy;
            if px >= icon_size || py >= icon_size {
                continue;
            }

            let local_x = dx as f32 + 0.5;
            let local_y = dy as f32 + 0.5;
            let inside_core = local_y >= radius && local_y <= height as f32 - radius;
            let top_center_y = radius;
            let bottom_center_y = height as f32 - radius;
            let circle_distance = if local_y < radius {
                ((local_x - radius).powi(2) + (local_y - top_center_y).powi(2)).sqrt()
            } else if local_y > height as f32 - radius {
                ((local_x - radius).powi(2) + (local_y - bottom_center_y).powi(2)).sqrt()
            } else {
                0.0
            };

            if inside_core || circle_distance <= radius {
                let idx = ((py * icon_size + px) * 4) as usize;
                rgba[idx] = color.r;
                rgba[idx + 1] = color.g;
                rgba[idx + 2] = color.b;
                rgba[idx + 3] = color.a;
            }
        }
    }
}

/// Computes the next action for a command-key toggle.
fn next_action(state: SessionState) -> ToggleAction {
    match state {
        SessionState::Idle => ToggleAction::Start,
        SessionState::Recording => ToggleAction::StopAndProcess,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppStatus, SessionState, ToggleAction, TrayIcons, WaveColor, WAVE_ICON_SIZE, next_action,
        render_wave_rgba,
    };

    #[test]
    fn idle_toggle_starts_recording() {
        assert_eq!(next_action(SessionState::Idle), ToggleAction::Start);
    }

    #[test]
    fn recording_toggle_stops_and_processes() {
        assert_eq!(
            next_action(SessionState::Recording),
            ToggleAction::StopAndProcess
        );
    }

    #[test]
    fn status_is_copyable() {
        let status = AppStatus::Listening;
        assert_eq!(status, AppStatus::Listening);
    }

    #[test]
    fn tray_icon_animations_have_expected_frame_counts() {
        let icons = TrayIcons::build().expect("tray icons should build");

        assert_eq!(icons.frames(AppStatus::Initializing).frame_count(), 3);
        assert_eq!(icons.frames(AppStatus::Idle).frame_count(), 1);
        assert_eq!(icons.frames(AppStatus::Listening).frame_count(), 4);
        assert_eq!(icons.frames(AppStatus::Processing).frame_count(), 5);
        assert_eq!(icons.frames(AppStatus::Error).frame_count(), 1);
    }

    #[test]
    fn listening_waveform_renders_red_pixels() {
        let rgba = render_wave_rgba([3, 7, 11, 7, 3], WaveColor::red());
        let first_opaque_pixel = rgba
            .chunks_exact(4)
            .find(|pixel| pixel[3] != 0)
            .expect("waveform should contain opaque pixels");

        assert_eq!(rgba.len(), (WAVE_ICON_SIZE * WAVE_ICON_SIZE * 4) as usize);
        assert_eq!(first_opaque_pixel, [255, 59, 48, 255]);
    }
}
