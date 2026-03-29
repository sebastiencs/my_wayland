use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::ErrorKind,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
};

use {
    anyhow::{anyhow, bail, Context},
    inotify::{Inotify, WatchDescriptor, WatchMask},
    serde::Deserialize,
    xkbcommon::xkb,
};

use crate::{
    clients::ClientsAction,
    drm::{schedule_outputs, schedule_outputs_for_points},
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    runner::{Dispatcher, Model, Redispatcher},
    state::{FractionalScale, State},
    xkb::{KeyboardState, Xkb},
};

const CONFIG_DIR_NAME: &str = "my_wayland";
const CONFIG_FILE_NAME: &str = "config.toml";
const WAYLAND_DISPLAY_NAME: &str = "my-wayland-0";
const DEFAULT_CURSOR_THEME: &str = "default";
const DEFAULT_CURSOR_SIZE: u32 = 24;
const INOTIFY_BUFFER_SIZE: usize = 4096;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputScaleSetting {
    Auto,
    Fixed(FractionalScale),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    path: PathBuf,
    pub general: GeneralConfig,
    pub bindings: Vec<Binding>,
    pub mouse_bindings: Vec<MouseBinding>,
    output_scales: BTreeMap<String, OutputScaleSetting>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralConfig {
    pub cursor_theme: String,
    pub cursor_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub trigger: KeyBinding,
    pub action: CompositorAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseBinding {
    pub trigger: MouseBindingTrigger,
    pub action: MouseBindingAction,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyBinding {
    pub description: String,
    pub keysym: xkb::Keysym,
    pub modifiers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MouseBindingTrigger {
    pub description: String,
    pub button: u32,
    pub modifiers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum CompositorAction {
    Spawn { command: String },
    Quit,
    ReloadConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum MouseBindingAction {
    MoveWindow,
    ResizeWindow,
}

#[derive(Debug)]
pub struct ConfigWatcher {
    inotify: Inotify,
    event_buffer: Box<[u8]>,
    poll_op_id: Option<RingId>,
    config_path: PathBuf,
    config_dir: PathBuf,
    watched_path: PathBuf,
    watched_descriptor: Option<WatchDescriptor>,
}

#[derive(Debug)]
pub enum ConfigWatcherAction {
    Start,
    Event,
    Failed { error: std::io::Error },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    general: GeneralConfigFile,
    bindings: BTreeMap<String, CompositorAction>,
    mouse_bindings: BTreeMap<String, MouseBindingAction>,
    output: OutputConfigFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OutputConfigFile {
    #[serde(default)]
    scales: BTreeMap<String, OutputScaleSettingFile>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OutputScaleSettingFile {
    Text(String),
    Number(f64),
}

fn output_scale_setting_from_file(
    value: OutputScaleSettingFile,
) -> anyhow::Result<OutputScaleSetting> {
    match value {
        OutputScaleSettingFile::Text(s) => {
            if s.eq_ignore_ascii_case("auto") {
                Ok(OutputScaleSetting::Auto)
            } else {
                let f: f64 = s
                    .parse()
                    .with_context(|| format!("invalid output scale `{s}`"))?;
                output_scale_setting_from_f64(f)
            }
        }
        OutputScaleSettingFile::Number(n) => output_scale_setting_from_f64(n),
    }
}

fn output_scale_setting_from_f64(scale: f64) -> anyhow::Result<OutputScaleSetting> {
    let Some(scale) = FractionalScale::try_from_f64(scale) else {
        bail!("output scale must be finite and positive");
    };
    Ok(OutputScaleSetting::Fixed(scale))
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GeneralConfigFile {
    cursor_theme: String,
    cursor_size: u32,
}

impl RuntimeConfig {
    pub fn load_initial(xkb: &Xkb) -> Self {
        let path = default_config_path();
        match Self::load_from_path(&path, xkb) {
            Ok(config) => config,
            Err(error) => {
                log::warn!(
                    "failed to load config from {}: {:#}. Using defaults.",
                    path.display(),
                    error
                );
                Self::default()
            }
        }
    }

    fn load_from_path(path: &Path, xkb: &Xkb) -> anyhow::Result<Self> {
        let file = match fs::read_to_string(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                log::info!("config file {} does not exist, using defaults", path.display());
                return Ok(Self::defaults_with_path(path.to_path_buf()));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read config file {}", path.display()));
            }
        };

        let parsed: ConfigFile =
            toml::from_str(&file).with_context(|| format!("failed to parse {}", path.display()))?;
        Self::compile(path.to_path_buf(), parsed, xkb)
    }

    fn compile(path: PathBuf, parsed: ConfigFile, xkb: &Xkb) -> anyhow::Result<Self> {
        let mut bindings = Vec::with_capacity(parsed.bindings.len());
        let mut seen = BTreeSet::new();
        for (description, action) in parsed.bindings {
            let trigger = parse_binding(&description, xkb.keymap())
                .with_context(|| format!("invalid binding `{description}`"))?;
            if !seen.insert((trigger.keysym, trigger.modifiers)) {
                bail!("duplicate binding for `{description}`");
            }
            bindings.push(Binding { trigger, action });
        }

        let mut mouse_bindings = Vec::with_capacity(parsed.mouse_bindings.len());
        let mut seen_mouse = BTreeSet::new();
        for (description, action) in parsed.mouse_bindings {
            let trigger = parse_mouse_binding(&description, xkb.keymap())
                .with_context(|| format!("invalid mouse binding `{description}`"))?;
            if !seen_mouse.insert((trigger.button, trigger.modifiers)) {
                bail!("duplicate mouse binding for `{description}`");
            }
            mouse_bindings.push(MouseBinding { trigger, action });
        }

        let mut output_scales = BTreeMap::new();
        for (name, value) in parsed.output.scales {
            let setting = output_scale_setting_from_file(value)
                .with_context(|| format!("invalid output scale for `{name}`"))?;
            output_scales.insert(name, setting);
        }

        Ok(Self {
            path,
            general: GeneralConfig {
                cursor_theme: sanitize_cursor_theme(parsed.general.cursor_theme),
                cursor_size: sanitize_cursor_size(parsed.general.cursor_size),
            },
            bindings,
            mouse_bindings,
            output_scales,
        })
    }

    fn defaults_with_path(path: PathBuf) -> Self {
        Self {
            path,
            general: GeneralConfig {
                cursor_theme: DEFAULT_CURSOR_THEME.to_string(),
                cursor_size: DEFAULT_CURSOR_SIZE,
            },
            bindings: Vec::new(),
            mouse_bindings: Vec::new(),
            output_scales: BTreeMap::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn output_scale(&self, output_name: &str) -> Option<OutputScaleSetting> {
        self.output_scales.get(output_name).copied()
    }

    pub fn binding_for_key(&self, keyboard: &KeyboardState, key: u32) -> Option<CompositorAction> {
        let keysym = keyboard.key_get_one_sym(key);
        if keysym.raw() == xkb::keysyms::KEY_NoSymbol {
            return None;
        }
        let modifiers = keyboard.effective_modifiers();
        self.bindings
            .iter()
            .find(|binding| binding.trigger.matches(keysym, modifiers))
            .map(|binding| binding.action.clone())
    }

    pub fn binding_for_pointer_button(
        &self,
        keyboard: &KeyboardState,
        button: u32,
    ) -> Option<MouseBindingAction> {
        let modifiers = keyboard.effective_modifiers();
        self.mouse_bindings
            .iter()
            .find(|binding| binding.trigger.matches(button, modifiers))
            .map(|binding| binding.action)
    }

    #[cfg(test)]
    pub(crate) fn parse_for_test(input: &str, xkb: &Xkb) -> anyhow::Result<Self> {
        let parsed: ConfigFile = toml::from_str(input)?;
        Self::compile(PathBuf::from("config.toml"), parsed, xkb)
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::defaults_with_path(default_config_path())
    }
}

impl GeneralConfig {
    pub fn default_cursor_theme() -> String {
        DEFAULT_CURSOR_THEME.to_string()
    }

    pub fn default_cursor_size() -> u32 {
        DEFAULT_CURSOR_SIZE
    }
}

impl KeyBinding {
    fn matches(&self, keysym: xkb::Keysym, modifiers: u32) -> bool {
        self.keysym == keysym && self.modifiers == modifiers
    }
}

impl MouseBindingTrigger {
    fn matches(&self, button: u32, modifiers: u32) -> bool {
        self.button == button && self.modifiers == modifiers
    }
}

impl Default for GeneralConfigFile {
    fn default() -> Self {
        Self {
            cursor_theme: GeneralConfig::default_cursor_theme(),
            cursor_size: GeneralConfig::default_cursor_size(),
        }
    }
}

impl ConfigWatcher {
    pub fn new(initial_state: &RuntimeConfig) -> Self {
        let config_path = initial_state.path().to_path_buf();
        let config_dir = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let watched_path = nearest_existing_directory(&config_dir);
        let inotify = Inotify::init().expect("failed to initialize config inotify watcher");
        let mut watcher = Self {
            inotify,
            event_buffer: vec![0; INOTIFY_BUFFER_SIZE].into_boxed_slice(),
            poll_op_id: None,
            config_path,
            config_dir,
            watched_path,
            watched_descriptor: None,
        };
        watcher
            .refresh_watch()
            .expect("failed to install config watcher");
        watcher
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        if self.poll_op_id.is_some() {
            return;
        }
        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.inotify.as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result: std::io::Result<PollCompleted>| {
                match result {
                    Ok(_) => ConfigWatcherAction::Event,
                    Err(error) => ConfigWatcherAction::Failed { error },
                }
            }),
        });
    }

    fn refresh_watch(&mut self) -> std::io::Result<bool> {
        let next_path = nearest_existing_directory(&self.config_dir);
        if self.watched_descriptor.is_some() && self.watched_path == next_path {
            return Ok(false);
        }

        if let Some(descriptor) = self.watched_descriptor.take() {
            let _ = self.inotify.watches().remove(descriptor);
        }

        let descriptor = self.inotify.watches().add(
            &next_path,
            WatchMask::CREATE
                | WatchMask::DELETE
                | WatchMask::CLOSE_WRITE
                | WatchMask::MOVED_FROM
                | WatchMask::MOVED_TO
                | WatchMask::DELETE_SELF
                | WatchMask::MOVE_SELF
                | WatchMask::ATTRIB,
        )?;
        self.watched_path = next_path;
        self.watched_descriptor = Some(descriptor);
        Ok(true)
    }

    fn on_event(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        let mut should_reload = false;
        let watching_config_dir = self.watched_path == self.config_dir;
        let config_dir_name = self.config_dir.file_name().map(|name| name.to_owned());

        loop {
            match self.inotify.read_events(self.event_buffer.as_mut()) {
                Ok(events) => {
                    let mut should_refresh_watch = false;
                    for event in events {
                        if event_targets_config(
                            &event,
                            watching_config_dir,
                            config_dir_name.as_deref(),
                        ) {
                            should_reload = true;
                        }
                        if event_requires_watch_refresh(
                            &event,
                            watching_config_dir,
                            config_dir_name.as_deref(),
                        ) {
                            should_refresh_watch = true;
                        }
                    }

                    if should_refresh_watch {
                        match self.refresh_watch() {
                            Ok(changed) => {
                                should_reload |= changed;
                            }
                            Err(error) => {
                                log::warn!("failed to refresh config watcher: {:?}", error);
                            }
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    log::warn!("failed to read config watcher events: {:?}", error);
                    break;
                }
            }
        }

        if should_reload {
            if let Err(error) = reload_from_disk(state, dispatcher, ReloadTrigger::AutomaticWatch) {
                log::warn!("failed to reload config after inotify event: {:#}", error);
            }
        }
    }
}

impl Model for ConfigWatcher {
    type Action = ConfigWatcherAction;
    const START: Option<Self::Action> = Some(ConfigWatcherAction::Start);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        match action {
            ConfigWatcherAction::Start => {
                self.dispatch_poll(dispatcher);
            }
            ConfigWatcherAction::Event => {
                self.on_event(state, dispatcher);
            }
            ConfigWatcherAction::Failed { error } => {
                self.poll_op_id = None;
                log::warn!("config watcher poll failed: {:?}", error);
                self.dispatch_poll(dispatcher);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ReloadTrigger {
    ManualBinding,
    AutomaticWatch,
}

pub fn reload_from_disk(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    trigger: ReloadTrigger,
) -> anyhow::Result<bool> {
    let path = state.config.path().to_path_buf();
    let next = RuntimeConfig::load_from_path(&path, &state.xkb)?;
    let changed = state.config != next;
    state.config = next;
    apply_general_settings(state, dispatcher);
    apply_output_settings(state, dispatcher);
    let trigger = match trigger {
        ReloadTrigger::ManualBinding => "manual binding",
        ReloadTrigger::AutomaticWatch => "file watch",
    };
    log::info!("reloaded config from {} via {}", path.display(), trigger);
    Ok(changed)
}

pub fn apply_general_settings(state: &mut State, dispatcher: &mut Dispatcher) {
    let general = state.config.general.clone();
    let changed = state
        .seat
        .set_cursor_theme(general.cursor_theme.clone(), general.cursor_size);
    if changed && state.seat.cursor_is_visible() {
        schedule_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
    }
}

pub fn apply_output_settings(state: &mut State, dispatcher: &mut Dispatcher) {
    let changed_outputs = state.apply_output_scales_from_config();
    if changed_outputs.is_empty() {
        return;
    }

    schedule_outputs(state, changed_outputs.iter().copied(), dispatcher);
    for output_id in changed_outputs {
        dispatcher.dispatch(ClientsAction::OutputStateChanged { output_id });
    }
}

pub fn execute_action(state: &mut State, dispatcher: &mut Dispatcher, action: CompositorAction) {
    match action {
        CompositorAction::Spawn { command } => {
            let result = std::process::Command::new("sh")
                .arg("-lc")
                .arg(&command)
                .env("WAYLAND_DISPLAY", WAYLAND_DISPLAY_NAME)
                .env_remove("DISPLAY")
                .spawn();
            if let Err(error) = result {
                log::warn!("failed to spawn `{}`: {:?}", command, error);
            }
        }
        CompositorAction::Quit => {
            log::info!("quitting compositor from keybinding");
            std::process::exit(0);
        }
        CompositorAction::ReloadConfig => {
            if let Err(error) = reload_from_disk(state, dispatcher, ReloadTrigger::ManualBinding) {
                log::warn!("failed to reload config: {:#}", error);
            }
        }
    }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        if !xdg_config_home.is_empty() {
            return PathBuf::from(xdg_config_home)
                .join(CONFIG_DIR_NAME)
                .join(CONFIG_FILE_NAME);
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join(CONFIG_DIR_NAME)
                .join(CONFIG_FILE_NAME);
        }
    }
    PathBuf::from(CONFIG_FILE_NAME)
}

fn parse_binding(description: &str, keymap: &xkb::Keymap) -> anyhow::Result<KeyBinding> {
    let parts: Vec<_> = description
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        bail!("binding is empty");
    }
    let (key_name, modifier_names) = parts.split_last().unwrap();
    let keysym_name = normalize_keysym_name(key_name);
    let keysym = xkb::keysym_from_name(&keysym_name, xkb::KEYSYM_CASE_INSENSITIVE);
    if keysym.raw() == xkb::keysyms::KEY_NoSymbol {
        bail!("unknown keysym `{}`", key_name);
    }
    let modifiers = parse_modifier_mask(modifier_names, keymap)?;
    Ok(KeyBinding { description: description.to_string(), keysym, modifiers })
}

fn parse_mouse_binding(
    description: &str,
    keymap: &xkb::Keymap,
) -> anyhow::Result<MouseBindingTrigger> {
    let parts: Vec<_> = description
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        bail!("binding is empty");
    }
    let (button_name, modifier_names) = parts.split_last().unwrap();
    let Some(button_name) = button_name.strip_prefix("mouse:") else {
        bail!("mouse binding must end with `mouse:<button>`");
    };
    let button = parse_mouse_button(button_name)?;
    let modifiers = parse_modifier_mask(modifier_names, keymap)?;
    Ok(MouseBindingTrigger { description: description.to_string(), button, modifiers })
}

fn parse_modifier_mask(modifier_names: &[&str], keymap: &xkb::Keymap) -> anyhow::Result<u32> {
    let mut modifiers = 0u32;
    for modifier_name in modifier_names {
        let canonical = normalize_modifier_name(modifier_name)
            .ok_or_else(|| anyhow!("unknown modifier `{}`", modifier_name))?;
        let index = keymap.mod_get_index(&canonical);
        if index == xkb::MOD_INVALID {
            bail!("modifier `{}` is not present in the current keymap", modifier_name);
        }
        if index >= u32::BITS {
            bail!("modifier `{}` index {} exceeds supported mask size", modifier_name, index);
        }
        modifiers |= 1u32 << index;
    }
    Ok(modifiers)
}

fn parse_mouse_button(name: &str) -> anyhow::Result<u32> {
    match name.to_ascii_lowercase().as_str() {
        "left" => Ok(BTN_LEFT),
        "right" => Ok(BTN_RIGHT),
        "middle" => Ok(BTN_MIDDLE),
        other => other
            .parse()
            .with_context(|| format!("unknown mouse button `{name}`")),
    }
}

fn normalize_modifier_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "shift" => Some(xkb::MOD_NAME_SHIFT),
        "ctrl" | "control" => Some(xkb::MOD_NAME_CTRL),
        "alt" => Some(xkb::MOD_NAME_ALT),
        "super" | "logo" | "mod4" => Some(xkb::MOD_NAME_LOGO),
        _ => None,
    }
}

fn normalize_keysym_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "enter" => "Return".to_string(),
        "esc" => "Escape".to_string(),
        "del" => "Delete".to_string(),
        "pgup" => "Page_Up".to_string(),
        "pgdown" => "Page_Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        other if other.len() == 1 => other.to_string(),
        _ => name.replace('-', "_"),
    }
}

fn sanitize_cursor_theme(theme: String) -> String {
    let trimmed = theme.trim();
    if trimmed.is_empty() {
        DEFAULT_CURSOR_THEME.to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_cursor_size(size: u32) -> u32 {
    size.max(1)
}

fn event_targets_config(
    event: &inotify::Event<&std::ffi::OsStr>,
    watching_config_dir: bool,
    config_dir_name: Option<&std::ffi::OsStr>,
) -> bool {
    if event.mask.contains(inotify::EventMask::Q_OVERFLOW) {
        return true;
    }

    if watching_config_dir {
        let Some(name) = event.name else {
            return event.mask.intersects(
                inotify::EventMask::DELETE_SELF
                    | inotify::EventMask::MOVE_SELF
                    | inotify::EventMask::ATTRIB,
            );
        };
        return name.as_bytes() == CONFIG_FILE_NAME.as_bytes()
            && event.mask.intersects(
                inotify::EventMask::CLOSE_WRITE
                    | inotify::EventMask::CREATE
                    | inotify::EventMask::DELETE
                    | inotify::EventMask::MOVED_FROM
                    | inotify::EventMask::MOVED_TO
                    | inotify::EventMask::ATTRIB,
            );
    }

    let Some(name) = event.name else {
        return false;
    };
    let Some(config_dir_name) = config_dir_name else {
        return false;
    };
    name == config_dir_name
        && event.mask.intersects(
            inotify::EventMask::CREATE
                | inotify::EventMask::DELETE
                | inotify::EventMask::MOVED_FROM
                | inotify::EventMask::MOVED_TO
                | inotify::EventMask::ATTRIB,
        )
}

fn event_requires_watch_refresh(
    event: &inotify::Event<&std::ffi::OsStr>,
    watching_config_dir: bool,
    config_dir_name: Option<&std::ffi::OsStr>,
) -> bool {
    if watching_config_dir {
        return event.mask.intersects(
            inotify::EventMask::DELETE_SELF
                | inotify::EventMask::MOVE_SELF
                | inotify::EventMask::IGNORED,
        );
    }
    event_targets_config(event, watching_config_dir, config_dir_name)
}

fn nearest_existing_directory(path: &Path) -> PathBuf {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.is_dir() {
            return candidate.to_path_buf();
        }
        current = candidate.parent();
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{egl::EGL, globals::Globals, runner::Dispatcher, seat::SeatState, state::State},
    };

    fn make_state(config: RuntimeConfig) -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config,
            seat: SeatState::default(),
            keyboard,
            xkb,
        }
    }

    #[test]
    fn parses_toml_binding_with_aliases() {
        let xkb = Xkb::new();
        let parsed: ConfigFile = toml::from_str(
            r#"
            [general]
            cursor_theme = "Adwaita"
            cursor_size = 32

            [bindings]
            "Super+Enter" = { action = "spawn", command = "alacritty" }
            "Ctrl+Shift+Esc" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let config = RuntimeConfig::compile(PathBuf::from("config.toml"), parsed, &xkb).unwrap();
        assert_eq!(config.general.cursor_theme, "Adwaita");
        assert_eq!(config.general.cursor_size, 32);
        assert_eq!(config.bindings.len(), 2);
        let enter = config
            .bindings
            .iter()
            .find(|binding| binding.trigger.description == "Super+Enter")
            .unwrap();
        let escape = config
            .bindings
            .iter()
            .find(|binding| binding.trigger.description == "Ctrl+Shift+Esc")
            .unwrap();
        assert_eq!(enter.trigger.keysym, xkb::keysym_from_name("Return", 0));
        assert_eq!(escape.trigger.keysym, xkb::keysym_from_name("Escape", 0));
    }

    #[test]
    fn rejects_duplicate_compiled_bindings() {
        let xkb = Xkb::new();
        let parsed: ConfigFile = toml::from_str(
            r#"
            [bindings]
            "Super+Enter" = { action = "reload-config" }
            "Mod4+Return" = { action = "spawn", command = "alacritty" }
            "#,
        )
        .unwrap();

        let error = RuntimeConfig::compile(PathBuf::from("config.toml"), parsed, &xkb).unwrap_err();
        assert!(error.to_string().contains("duplicate binding"));
    }

    #[test]
    fn parses_mouse_bindings_with_aliases() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "Alt+mouse:273" = { action = "resize-window" }
            "#,
            &xkb,
        )
        .unwrap();

        assert_eq!(config.mouse_bindings.len(), 2);
        let move_binding = config
            .mouse_bindings
            .iter()
            .find(|binding| binding.trigger.description == "Super+mouse:left")
            .unwrap();
        let resize_binding = config
            .mouse_bindings
            .iter()
            .find(|binding| binding.trigger.description == "Alt+mouse:273")
            .unwrap();
        assert_eq!(move_binding.trigger.button, BTN_LEFT);
        assert_eq!(move_binding.action, MouseBindingAction::MoveWindow);
        assert_eq!(resize_binding.trigger.button, BTN_RIGHT);
        assert_eq!(resize_binding.action, MouseBindingAction::ResizeWindow);
    }

    #[test]
    fn matches_mouse_binding_for_pressed_modifiers() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &xkb,
        )
        .unwrap();
        let mut keyboard = xkb.create_keyboard_state();

        assert_eq!(config.binding_for_pointer_button(&keyboard, BTN_LEFT), None);
        keyboard.update_key(125, true);
        assert_eq!(
            config.binding_for_pointer_button(&keyboard, BTN_LEFT),
            Some(MouseBindingAction::MoveWindow)
        );
    }

    #[test]
    fn rejects_duplicate_mouse_bindings() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "Mod4+mouse:272" = { action = "resize-window" }
            "#,
            &xkb,
        );

        let error = config.unwrap_err();
        assert!(error.to_string().contains("duplicate mouse binding"));
    }

    #[test]
    fn reload_keeps_last_good_config_when_new_file_is_invalid() {
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let path = temp_dir.to_path_buf().join("config.toml");
        fs::write(
            &path,
            r#"
            [general]
            cursor_theme = "Adwaita"

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let mut state = make_state(config.clone());
        state.config.path = path.clone();
        let mut dispatcher = Dispatcher::default();

        fs::write(&path, "[bindings\n").unwrap();

        let error = reload_from_disk(&mut state, &mut dispatcher, ReloadTrigger::ManualBinding)
            .unwrap_err();
        assert!(error.to_string().contains("failed to parse"));
        assert_eq!(state.config, config);
    }
}
