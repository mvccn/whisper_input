//! Runtime orchestration for tray UI, hotkey control, audio capture, Whisper, and output.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tracing::{error, info, warn};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::audio::{ActiveRecording, Recorder, has_minimum_signal, to_whisper_samples};
use crate::config::Config;
use crate::hotkey::{self, HotkeyEvent};
use crate::model::{self, ModelSize};
use crate::paste;
use crate::sound::{self, SoundCue};
use crate::transcribe::WhisperEngine;

const MENU_ID_TOGGLE: &str = "toggle";
const MENU_ID_MODEL_TINY: &str = "model_tiny";
const MENU_ID_MODEL_BASE: &str = "model_base";
const MENU_ID_MODEL_SMALL: &str = "model_small";
const MENU_ID_MODEL_MEDIUM: &str = "model_medium";
const MENU_ID_MODEL_LARGE: &str = "model_large";
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerCommand {
    ToggleRecording,
    SetModelSize(ModelSize),
    Quit,
}

/// Notifications sent from the worker back to the tray UI.
#[derive(Debug, Clone)]
enum WorkerEvent {
    Status(AppStatus),
    Error(String),
    ModelSizeChanged(ModelSize),
    PasteRequested,
    TranscriptReady(usize),
}

/// Custom tao event payload for worker and menu notifications.
#[derive(Debug, Clone)]
enum UserEvent {
    Worker(WorkerEvent),
    Menu(MenuEvent),
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
    let worker_tx = spawn_worker(config, proxy.clone());
    let mut tray_ui = TrayUi::new(initial_model_size)?;

    event_loop.run(move |event, _event_loop, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                info!("menu bar app started");
            }
            Event::UserEvent(UserEvent::Worker(worker_event)) => {
                if let Err(err) = apply_worker_event(worker_event, &mut tray_ui) {
                    error!(error = %err, "failed to update tray state");
                }
            }
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                if handle_menu_event(&menu_event, &worker_tx) {
                    *control_flow = ControlFlow::Exit;
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
fn apply_worker_event(event: WorkerEvent, tray_ui: &mut TrayUi) -> Result<()> {
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
fn handle_menu_event(event: &MenuEvent, worker_tx: &Sender<WorkerCommand>) -> bool {
    if let Some(model_size) = model_size_from_menu_id(&event.id) {
        if worker_tx
            .send(WorkerCommand::SetModelSize(model_size))
            .is_err()
        {
            warn!("worker command channel closed while setting model size");
            return true;
        }
        return false;
    }

    if event.id == MENU_ID_TOGGLE {
        if worker_tx.send(WorkerCommand::ToggleRecording).is_err() {
            warn!("worker command channel closed while toggling recording");
            return true;
        }
        return false;
    }

    if event.id == MENU_ID_QUIT {
        let _ = worker_tx.send(WorkerCommand::Quit);
        return true;
    }

    false
}

/// Maps a menu event identifier to a model size preset.
fn model_size_from_menu_id(menu_id: &MenuId) -> Option<ModelSize> {
    if menu_id == MENU_ID_MODEL_TINY {
        return Some(ModelSize::Tiny);
    }
    if menu_id == MENU_ID_MODEL_BASE {
        return Some(ModelSize::Base);
    }
    if menu_id == MENU_ID_MODEL_SMALL {
        return Some(ModelSize::Small);
    }
    if menu_id == MENU_ID_MODEL_MEDIUM {
        return Some(ModelSize::Medium);
    }
    if menu_id == MENU_ID_MODEL_LARGE {
        return Some(ModelSize::Large);
    }

    None
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
    hotkey::spawn_listener(hotkey_tx, Duration::from_millis(config.hotkey_max_tap_ms));

    let mut runtime = initialize_runtime(&config, &proxy);

    loop {
        while let Ok(hotkey_event) = hotkey_rx.try_recv() {
            if hotkey_event == HotkeyEvent::ToggleRecording {
                handle_toggle_request(&mut runtime, &config, &proxy);
            }
        }

        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(WorkerCommand::ToggleRecording) => {
                handle_toggle_request(&mut runtime, &config, &proxy);
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
    model_tiny_item: CheckMenuItem,
    model_base_item: CheckMenuItem,
    model_small_item: CheckMenuItem,
    model_medium_item: CheckMenuItem,
    model_large_item: CheckMenuItem,
    icons: TrayIcons,
}

impl TrayUi {
    /// Builds the tray menu and initial icon state.
    ///
    /// # Errors
    /// Returns an error when icon conversion or tray setup fails.
    fn new(initial_model_size: ModelSize) -> Result<Self> {
        let icons = TrayIcons::build().context("failed to build tray icons")?;

        let menu = Menu::new();
        let status_item = MenuItem::new("Status: Initializing", false, None);
        let separator1 = PredefinedMenuItem::separator();
        let toggle_item = MenuItem::with_id(MENU_ID_TOGGLE, "Start Listening", true, None);
        let separator2 = PredefinedMenuItem::separator();
        let model_tiny_item = CheckMenuItem::with_id(
            MENU_ID_MODEL_TINY,
            "Tiny (fastest)",
            true,
            initial_model_size == ModelSize::Tiny,
            None,
        );
        let model_base_item = CheckMenuItem::with_id(
            MENU_ID_MODEL_BASE,
            "Base (default)",
            true,
            initial_model_size == ModelSize::Base,
            None,
        );
        let model_small_item = CheckMenuItem::with_id(
            MENU_ID_MODEL_SMALL,
            "Small",
            true,
            initial_model_size == ModelSize::Small,
            None,
        );
        let model_medium_item = CheckMenuItem::with_id(
            MENU_ID_MODEL_MEDIUM,
            "Medium",
            true,
            initial_model_size == ModelSize::Medium,
            None,
        );
        let model_large_item = CheckMenuItem::with_id(
            MENU_ID_MODEL_LARGE,
            "Large (slowest)",
            true,
            initial_model_size == ModelSize::Large,
            None,
        );
        let model_size_menu = Submenu::with_items(
            "Model Size",
            true,
            &[
                &model_tiny_item,
                &model_base_item,
                &model_small_item,
                &model_medium_item,
                &model_large_item,
            ],
        )
        .context("failed to build model-size submenu")?;
        let separator3 = PredefinedMenuItem::separator();
        let quit_item = MenuItem::with_id(MENU_ID_QUIT, "Quit", true, None);

        menu.append_items(&[
            &status_item,
            &separator1,
            &toggle_item,
            &separator2,
            &model_size_menu,
            &separator3,
            &quit_item,
        ])
        .context("failed to build tray menu items")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("whisper_input")
            .with_icon(icons.initializing.clone())
            .with_icon_as_template(false)
            .build()
            .context("failed to create tray icon")?;

        Ok(Self {
            tray_icon,
            status_item,
            toggle_item,
            model_tiny_item,
            model_base_item,
            model_small_item,
            model_medium_item,
            model_large_item,
            icons,
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
                self.status_item.set_text(format!("Error: {message}"));
            }
            WorkerEvent::ModelSizeChanged(model_size) => {
                self.apply_model_size(model_size);
            }
            WorkerEvent::PasteRequested => {}
            WorkerEvent::TranscriptReady(char_count) => {
                self.status_item
                    .set_text(format!("Transcript ready ({char_count} chars)"));
            }
        }

        Ok(())
    }

    /// Updates the tray icon and action label for a status state.
    ///
    /// # Errors
    /// Returns an error when icon updates fail.
    fn apply_status(&mut self, status: AppStatus) -> Result<()> {
        match status {
            AppStatus::Initializing => {
                self.tray_icon
                    .set_icon(Some(self.icons.initializing.clone()))
                    .context("failed to set initializing icon")?;
                self.status_item.set_text("Status: Initializing");
                self.toggle_item.set_text("Start Listening");
            }
            AppStatus::Idle => {
                self.tray_icon
                    .set_icon(Some(self.icons.idle.clone()))
                    .context("failed to set idle icon")?;
                self.status_item.set_text("Status: Idle");
                self.toggle_item.set_text("Start Listening");
            }
            AppStatus::Listening => {
                self.tray_icon
                    .set_icon(Some(self.icons.listening.clone()))
                    .context("failed to set listening icon")?;
                self.status_item.set_text("Status: Listening");
                self.toggle_item.set_text("Stop Listening");
            }
            AppStatus::Processing => {
                self.tray_icon
                    .set_icon(Some(self.icons.processing.clone()))
                    .context("failed to set processing icon")?;
                self.status_item.set_text("Status: Processing");
                self.toggle_item.set_text("Processing...");
            }
            AppStatus::Error => {
                self.tray_icon
                    .set_icon(Some(self.icons.error.clone()))
                    .context("failed to set error icon")?;
                self.toggle_item.set_text("Retry Start Listening");
            }
        }

        Ok(())
    }

    /// Updates model-size checkmarks in the submenu.
    fn apply_model_size(&self, model_size: ModelSize) {
        self.model_tiny_item
            .set_checked(model_size == ModelSize::Tiny);
        self.model_base_item
            .set_checked(model_size == ModelSize::Base);
        self.model_small_item
            .set_checked(model_size == ModelSize::Small);
        self.model_medium_item
            .set_checked(model_size == ModelSize::Medium);
        self.model_large_item
            .set_checked(model_size == ModelSize::Large);
    }
}

/// Pre-rendered RGBA icons for tray status transitions.
struct TrayIcons {
    initializing: Icon,
    idle: Icon,
    listening: Icon,
    processing: Icon,
    error: Icon,
}

impl TrayIcons {
    /// Builds all tray icons from small RGBA circles.
    ///
    /// # Errors
    /// Returns an error when any icon buffer cannot be converted.
    fn build() -> Result<Self> {
        Ok(Self {
            initializing: build_circle_icon(180, 180, 180)?,
            idle: build_circle_icon(120, 120, 120)?,
            listening: build_circle_icon(210, 48, 64)?,
            processing: build_circle_icon(52, 152, 219)?,
            error: build_circle_icon(245, 166, 35)?,
        })
    }
}

/// Creates a small circular RGBA status icon.
///
/// # Errors
/// Returns an error when the raw RGBA buffer is rejected by tray-icon.
fn build_circle_icon(r: u8, g: u8, b: u8) -> Result<Icon> {
    const SIZE: u32 = 18;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];

    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = SIZE as f32 * 0.35;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= radius {
                let idx = ((y * SIZE + x) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).context("invalid RGBA icon buffer")
}

/// Computes the next action for a left-command toggle.
fn next_action(state: SessionState) -> ToggleAction {
    match state {
        SessionState::Idle => ToggleAction::Start,
        SessionState::Recording => ToggleAction::StopAndProcess,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppStatus, MENU_ID_MODEL_BASE, MENU_ID_MODEL_LARGE, MENU_ID_MODEL_MEDIUM,
        MENU_ID_MODEL_SMALL, MENU_ID_MODEL_TINY, SessionState, ToggleAction,
        model_size_from_menu_id, next_action,
    };
    use crate::model::ModelSize;

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
    fn model_size_menu_mapping_matches_all_entries() {
        assert_eq!(
            model_size_from_menu_id(&MENU_ID_MODEL_TINY.into()),
            Some(ModelSize::Tiny)
        );
        assert_eq!(
            model_size_from_menu_id(&MENU_ID_MODEL_BASE.into()),
            Some(ModelSize::Base)
        );
        assert_eq!(
            model_size_from_menu_id(&MENU_ID_MODEL_SMALL.into()),
            Some(ModelSize::Small)
        );
        assert_eq!(
            model_size_from_menu_id(&MENU_ID_MODEL_MEDIUM.into()),
            Some(ModelSize::Medium)
        );
        assert_eq!(
            model_size_from_menu_id(&MENU_ID_MODEL_LARGE.into()),
            Some(ModelSize::Large)
        );
        assert_eq!(model_size_from_menu_id(&"unknown_model".into()), None);
    }
}
