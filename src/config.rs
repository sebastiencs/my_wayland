use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    ffi::OsStr,
    fmt, fs,
    io::ErrorKind,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use {
    anyhow::{anyhow, bail, Context},
    inotify::{EventMask, Inotify, WatchDescriptor, WatchMask},
    serde::Deserialize,
    xkbcommon::xkb,
};

use crate::{
    clients::{ClientManagerAction, Resolved, WAYLAND_DISPLAY_NAME},
    drm::{kick_visual_animation_pump, request_outputs, request_outputs_for_points},
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    protocol_generated::WlSurfaceId,
    runner::{Dispatcher, Model, Redispatcher, ShutdownReason},
    state::{FractionalScale, ManagedWindowMode, State, WorkspaceGroupId, WorkspaceId},
    xkb::{KeyboardState, Xkb},
};

const CONFIG_DIR_NAME: &str = "my_wayland";
const CONFIG_FILE_NAME: &str = "config.toml";
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
    pub decorations: DecorationsConfig,
    pub effects: EffectsConfig,
    pub animations: AnimationsConfig,
    pub bindings: Vec<Binding>,
    pub mouse_bindings: Vec<MouseBinding>,
    output_scales: BTreeMap<String, OutputScaleSetting>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralConfig {
    pub cursor_theme: String,
    pub cursor_size: u32,
    pub outer_gap: i32,
    pub inner_gap: i32,
    pub decoration_insets: DecorationInsets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum XdgDecorationModePolicy {
    Server,
    Client,
    HonorClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecorationsConfig {
    pub xdg: XdgDecorationConfig,
    pub border: BorderDecorationConfig,
    pub shadow: ShadowDecorationConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XdgDecorationConfig {
    pub no_protocol: XdgDecorationModePolicy,
    pub tiled: XdgDecorationModePolicy,
    pub floating: XdgDecorationModePolicy,
    pub fullscreen: XdgDecorationModePolicy,
    pub unset_mode: XdgDecorationModePolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderDecorationConfig {
    pub thickness: i32,
    pub active_start_color: RgbaColor,
    pub active_end_color: RgbaColor,
    pub inactive_start_color: RgbaColor,
    pub inactive_end_color: RgbaColor,
    pub gradient_angle_degrees: i32,
    pub rounding: i32,
    pub rounding_power: RoundingPower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowDecorationConfig {
    pub radius: i32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub color: RgbaColor,
    pub power: ShadowPower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectsConfig {
    pub blur: BlurConfig,
    pub opacity: OpacityConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurConfig {
    pub enabled: bool,
    pub radius: i32,
    pub passes: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpacityConfig {
    pub active_alpha: u8,
    pub inactive_alpha: u8,
    pub inactive_dim: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationsConfig {
    pub window: WindowAnimationConfig,
    pub workspace: WorkspaceAnimationConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowAnimationConfig {
    pub duration_msec: u32,
    pub open_duration_msec: u32,
    pub close_duration_msec: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceAnimationConfig {
    pub duration_msec: u32,
    pub fade: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecorationInsets {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Copy, Debug, PartialOrd)]
pub struct RoundingPower(f32);

#[derive(Clone, Copy, Debug, PartialOrd)]
pub struct ShadowPower(f32);

impl RoundingPower {
    pub const DEFAULT: Self = Self(2.0);

    pub fn sanitize(raw: f32) -> Self {
        if raw.is_finite() && raw > 0.0 {
            Self(raw)
        } else {
            Self::DEFAULT
        }
    }

    pub const fn get(self) -> f32 {
        self.0
    }
}

impl Default for RoundingPower {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl PartialEq for RoundingPower {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for RoundingPower {}

impl ShadowPower {
    pub const DEFAULT: Self = Self(2.0);

    pub fn sanitize(raw: f32) -> Self {
        if raw.is_finite() && raw > 0.0 {
            Self(raw)
        } else {
            Self::DEFAULT
        }
    }

    pub const fn get(self) -> f32 {
        self.0
    }
}

impl Default for ShadowPower {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl PartialEq for ShadowPower {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for ShadowPower {}

impl RgbaColor {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn to_rgba_bytes(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn to_premultiplied_rgba_bytes(self) -> [u8; 4] {
        let alpha = self.a as u16;
        let premultiply = |channel: u8| -> u8 { (((channel as u16) * alpha + 127) / 255) as u8 };
        [premultiply(self.r), premultiply(self.g), premultiply(self.b), self.a]
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositorAction {
    Spawn {
        command: String,
    },
    Quit,
    ReloadConfig,
    WorkspaceNext,
    WorkspacePrevious,
    Close,
    FocusNext,
    FocusPrevious,
    ToggleFloating,
    ToggleFullscreen,
    /// One-based workspace on the active output's group.
    WorkspaceIndex {
        index: u32,
    },
    /// One-based destination workspace on the active output's group.
    MoveToWorkspace {
        index: u32,
    },
}

/// Tagged binding table. The TOML shape stays `{ action = "...", ... }`. A struct is stricter
/// than serde's internally tagged enums, which ignore fields the selected variant does not
/// declare.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionFile {
    action: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    index: Option<u32>,
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
    watches: HashMap<WatchDescriptor, ConfigWatchSlot>,
    stopped: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigWatchSlot {
    /// `config_path` without following symlinks (Hyprland-style `IN_DONT_FOLLOW`).
    LogicalFile,
    /// Symlink target from `canonicalize`, when distinct from the logical path.
    ResolvedTarget,
    /// Ancestor directory used to observe create/delete/rename of the config entry.
    ParentDirectory,
}

#[derive(Debug)]
pub enum ConfigWatcherAction {
    Start,
    Event,
    Failed { error: std::io::Error },
    Stop,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    general: GeneralConfigFile,
    decorations: DecorationsConfigFile,
    effects: EffectsConfigFile,
    animations: AnimationsConfigFile,
    bindings: BTreeMap<String, ActionFile>,
    mouse_bindings: BTreeMap<String, MouseBindingAction>,
    output: OutputConfigFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DecorationsConfigFile {
    xdg: XdgDecorationConfigFile,
    border: BorderDecorationConfigFile,
    shadow: ShadowDecorationConfigFile,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct XdgDecorationConfigFile {
    no_protocol: XdgDecorationModePolicy,
    tiled: XdgDecorationModePolicy,
    floating: XdgDecorationModePolicy,
    fullscreen: XdgDecorationModePolicy,
    unset_mode: XdgDecorationModePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BorderDecorationConfigFile {
    thickness: i32,
    active_start_color: String,
    active_end_color: String,
    inactive_start_color: String,
    inactive_end_color: String,
    gradient_angle_degrees: i32,
    rounding: i32,
    rounding_power: f32,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ShadowDecorationConfigFile {
    radius: i32,
    offset_x: i32,
    offset_y: i32,
    color: String,
    power: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EffectsConfigFile {
    blur: BlurConfigFile,
    opacity: OpacityConfigFile,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BlurConfigFile {
    enabled: bool,
    radius: i32,
    passes: u8,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OpacityConfigFile {
    active_alpha: u8,
    inactive_alpha: u8,
    inactive_dim: u8,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AnimationsConfigFile {
    window: WindowAnimationConfigFile,
    workspace: WorkspaceAnimationConfigFile,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WindowAnimationConfigFile {
    duration_msec: u32,
    open_duration_msec: u32,
    close_duration_msec: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkspaceAnimationConfigFile {
    duration_msec: u32,
    fade: bool,
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
    outer_gap: i32,
    inner_gap: i32,
    decoration_insets: DecorationInsets,
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
            let action = compositor_action_from_file(&description, action)?;
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

        let border = BorderDecorationConfig {
            thickness: sanitize_non_negative_i32(parsed.decorations.border.thickness),
            active_start_color: parse_rgba_color(&parsed.decorations.border.active_start_color)
                .with_context(|| "invalid decorations.border.active_start_color")?,
            active_end_color: parse_rgba_color(&parsed.decorations.border.active_end_color)
                .with_context(|| "invalid decorations.border.active_end_color")?,
            inactive_start_color: parse_rgba_color(&parsed.decorations.border.inactive_start_color)
                .with_context(|| "invalid decorations.border.inactive_start_color")?,
            inactive_end_color: parse_rgba_color(&parsed.decorations.border.inactive_end_color)
                .with_context(|| "invalid decorations.border.inactive_end_color")?,
            gradient_angle_degrees: parsed.decorations.border.gradient_angle_degrees,
            rounding: sanitize_non_negative_i32(parsed.decorations.border.rounding),
            rounding_power: RoundingPower::sanitize(parsed.decorations.border.rounding_power),
        };
        let shadow = ShadowDecorationConfig {
            radius: sanitize_non_negative_i32(parsed.decorations.shadow.radius),
            offset_x: parsed.decorations.shadow.offset_x,
            offset_y: parsed.decorations.shadow.offset_y,
            color: parse_rgba_color(&parsed.decorations.shadow.color)
                .with_context(|| "invalid decorations.shadow.color")?,
            power: ShadowPower::sanitize(parsed.decorations.shadow.power),
        };
        let effects = EffectsConfig {
            blur: BlurConfig {
                enabled: parsed.effects.blur.enabled,
                radius: sanitize_non_negative_i32(parsed.effects.blur.radius),
                passes: parsed.effects.blur.passes.max(1),
            },
            opacity: OpacityConfig {
                active_alpha: parsed.effects.opacity.active_alpha,
                inactive_alpha: parsed.effects.opacity.inactive_alpha,
                inactive_dim: parsed.effects.opacity.inactive_dim,
            },
        };
        let animations = AnimationsConfig {
            window: WindowAnimationConfig {
                duration_msec: parsed.animations.window.duration_msec,
                open_duration_msec: parsed.animations.window.open_duration_msec,
                close_duration_msec: parsed.animations.window.close_duration_msec,
            },
            workspace: WorkspaceAnimationConfig {
                duration_msec: parsed.animations.workspace.duration_msec,
                fade: parsed.animations.workspace.fade,
            },
        };

        Ok(Self {
            path,
            general: GeneralConfig {
                cursor_theme: sanitize_cursor_theme(parsed.general.cursor_theme),
                cursor_size: sanitize_cursor_size(parsed.general.cursor_size),
                outer_gap: sanitize_non_negative_i32(parsed.general.outer_gap),
                inner_gap: sanitize_non_negative_i32(parsed.general.inner_gap),
                decoration_insets: sanitize_decoration_insets(parsed.general.decoration_insets),
            },
            decorations: DecorationsConfig {
                xdg: XdgDecorationConfig {
                    no_protocol: parsed.decorations.xdg.no_protocol,
                    tiled: parsed.decorations.xdg.tiled,
                    floating: parsed.decorations.xdg.floating,
                    fullscreen: parsed.decorations.xdg.fullscreen,
                    unset_mode: parsed.decorations.xdg.unset_mode,
                },
                border,
                shadow,
            },
            effects,
            animations,
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
                outer_gap: 0,
                inner_gap: 0,
                decoration_insets: DecorationInsets::default(),
            },
            decorations: DecorationsConfig::default(),
            effects: EffectsConfig::default(),
            animations: AnimationsConfig::default(),
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

    pub fn default_outer_gap() -> i32 {
        0
    }

    pub fn default_inner_gap() -> i32 {
        0
    }
}

impl Default for DecorationsConfig {
    fn default() -> Self {
        Self {
            xdg: XdgDecorationConfig::default(),
            border: BorderDecorationConfig::default(),
            shadow: ShadowDecorationConfig::default(),
        }
    }
}

impl Default for XdgDecorationConfig {
    fn default() -> Self {
        Self {
            no_protocol: XdgDecorationModePolicy::Client,
            tiled: XdgDecorationModePolicy::Server,
            floating: XdgDecorationModePolicy::HonorClient,
            fullscreen: XdgDecorationModePolicy::Server,
            unset_mode: XdgDecorationModePolicy::Server,
        }
    }
}

impl Default for BorderDecorationConfig {
    fn default() -> Self {
        Self {
            thickness: 0,
            active_start_color: RgbaColor::new(0x50, 0x78, 0xc8, 0xff),
            active_end_color: RgbaColor::new(0x6b, 0x9d, 0xf6, 0xff),
            inactive_start_color: RgbaColor::new(0x33, 0x38, 0x44, 0xff),
            inactive_end_color: RgbaColor::new(0x43, 0x48, 0x54, 0xff),
            gradient_angle_degrees: 0,
            rounding: 0,
            rounding_power: RoundingPower::DEFAULT,
        }
    }
}

impl Default for ShadowDecorationConfig {
    fn default() -> Self {
        Self {
            radius: 0,
            offset_x: 0,
            offset_y: 8,
            color: RgbaColor::new(0, 0, 0, 0x90),
            power: ShadowPower::DEFAULT,
        }
    }
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self {
            blur: BlurConfig::default(),
            opacity: OpacityConfig::default(),
        }
    }
}

impl Default for BlurConfig {
    fn default() -> Self {
        Self { enabled: false, radius: 12, passes: 2 }
    }
}

impl Default for OpacityConfig {
    fn default() -> Self {
        Self { active_alpha: u8::MAX, inactive_alpha: 230, inactive_dim: 24 }
    }
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            window: WindowAnimationConfig::default(),
            workspace: WorkspaceAnimationConfig::default(),
        }
    }
}

impl Default for WindowAnimationConfig {
    fn default() -> Self {
        Self {
            duration_msec: 160,
            open_duration_msec: 160,
            close_duration_msec: 160,
        }
    }
}

impl Default for WorkspaceAnimationConfig {
    fn default() -> Self {
        Self { duration_msec: 250, fade: true }
    }
}

impl Default for XdgDecorationModePolicy {
    fn default() -> Self {
        Self::HonorClient
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
            outer_gap: GeneralConfig::default_outer_gap(),
            inner_gap: GeneralConfig::default_inner_gap(),
            decoration_insets: DecorationInsets::default(),
        }
    }
}

impl Default for XdgDecorationConfigFile {
    fn default() -> Self {
        let defaults = XdgDecorationConfig::default();
        Self {
            no_protocol: defaults.no_protocol,
            tiled: defaults.tiled,
            floating: defaults.floating,
            fullscreen: defaults.fullscreen,
            unset_mode: defaults.unset_mode,
        }
    }
}

impl Default for BorderDecorationConfigFile {
    fn default() -> Self {
        let defaults = BorderDecorationConfig::default();
        Self {
            thickness: defaults.thickness,
            active_start_color: "#5078c8ff".to_string(),
            active_end_color: "#6b9df6ff".to_string(),
            inactive_start_color: "#333844ff".to_string(),
            inactive_end_color: "#434854ff".to_string(),
            gradient_angle_degrees: defaults.gradient_angle_degrees,
            rounding: defaults.rounding,
            rounding_power: defaults.rounding_power.get(),
        }
    }
}

impl Default for ShadowDecorationConfigFile {
    fn default() -> Self {
        let defaults = ShadowDecorationConfig::default();
        Self {
            radius: defaults.radius,
            offset_x: defaults.offset_x,
            offset_y: defaults.offset_y,
            color: "#00000090".to_string(),
            power: defaults.power.get(),
        }
    }
}

impl Default for BlurConfigFile {
    fn default() -> Self {
        let defaults = BlurConfig::default();
        Self {
            enabled: defaults.enabled,
            radius: defaults.radius,
            passes: defaults.passes,
        }
    }
}

impl Default for OpacityConfigFile {
    fn default() -> Self {
        let defaults = OpacityConfig::default();
        Self {
            active_alpha: defaults.active_alpha,
            inactive_alpha: defaults.inactive_alpha,
            inactive_dim: defaults.inactive_dim,
        }
    }
}

impl Default for WindowAnimationConfigFile {
    fn default() -> Self {
        Self {
            duration_msec: WindowAnimationConfig::default().duration_msec,
            open_duration_msec: WindowAnimationConfig::default().open_duration_msec,
            close_duration_msec: WindowAnimationConfig::default().close_duration_msec,
        }
    }
}

impl Default for WorkspaceAnimationConfigFile {
    fn default() -> Self {
        Self {
            duration_msec: WorkspaceAnimationConfig::default().duration_msec,
            fade: WorkspaceAnimationConfig::default().fade,
        }
    }
}

impl ConfigWatcher {
    pub fn try_new(initial_state: &RuntimeConfig) -> std::io::Result<Self> {
        let inotify = Inotify::init()?;
        let mut watcher = Self {
            inotify,
            event_buffer: vec![0; INOTIFY_BUFFER_SIZE].into_boxed_slice(),
            poll_op_id: None,
            watches: HashMap::new(),
            stopped: false,
        };
        watcher.sync_watches(initial_state.path())?;
        Ok(watcher)
    }

    pub fn new(initial_state: &RuntimeConfig) -> Self {
        Self::try_new(initial_state).expect("failed to initialize config inotify watcher")
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        if self.stopped || self.poll_op_id.is_some() {
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

    /// Drop every watch and subscribe again from `config_path` (logical path).
    fn sync_watches(&mut self, config_path: &Path) -> std::io::Result<()> {
        for descriptor in self.watches.keys().cloned().collect::<Vec<_>>() {
            let _ = self.inotify.watches().remove(descriptor);
        }
        self.watches.clear();

        let Some(config_dir) = config_path.parent() else {
            return Ok(());
        };
        let parent_watch_path = nearest_existing_directory(config_dir);
        let parent_mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::CLOSE_WRITE
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::DELETE_SELF
            | WatchMask::MOVE_SELF
            | WatchMask::ATTRIB;
        let descriptor = self
            .inotify
            .watches()
            .add(&parent_watch_path, parent_mask)?;
        self.watches
            .insert(descriptor, ConfigWatchSlot::ParentDirectory);

        let Ok(meta) = fs::symlink_metadata(config_path) else {
            return Ok(());
        };

        let logical_mask = WatchMask::MODIFY
            | WatchMask::CLOSE_WRITE
            | WatchMask::ATTRIB
            | WatchMask::DELETE_SELF
            | WatchMask::MOVE_SELF
            | WatchMask::DONT_FOLLOW;
        let descriptor = self.inotify.watches().add(config_path, logical_mask)?;
        self.watches
            .insert(descriptor, ConfigWatchSlot::LogicalFile);

        if meta.file_type().is_symlink() {
            if let Ok(canonical) = fs::canonicalize(config_path) {
                if canonical != config_path {
                    let target_mask = WatchMask::MODIFY
                        | WatchMask::CLOSE_WRITE
                        | WatchMask::ATTRIB
                        | WatchMask::DELETE_SELF
                        | WatchMask::MOVE_SELF;
                    let descriptor = self.inotify.watches().add(&canonical, target_mask)?;
                    self.watches
                        .insert(descriptor, ConfigWatchSlot::ResolvedTarget);
                }
            }
        }

        Ok(())
    }

    fn on_event(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        let config_path = state.config.path().to_path_buf();
        let config_file_name = config_path.file_name().map(|name| name.to_owned());

        let mut should_reload = false;
        let mut should_resync = false;

        loop {
            match self.inotify.read_events(self.event_buffer.as_mut()) {
                Ok(events) => {
                    for event in events {
                        if event.mask.contains(EventMask::Q_OVERFLOW) {
                            should_reload = true;
                            should_resync = true;
                            continue;
                        }

                        if event.mask.contains(EventMask::IGNORED) {
                            if !self.watches.contains_key(&event.wd) {
                                continue;
                            }
                            should_resync = true;
                            continue;
                        }

                        let Some(slot) = self.watches.get(&event.wd).copied() else {
                            continue;
                        };

                        match slot {
                            ConfigWatchSlot::LogicalFile | ConfigWatchSlot::ResolvedTarget => {
                                if event.mask.intersects(
                                    EventMask::MODIFY
                                        | EventMask::CLOSE_WRITE
                                        | EventMask::ATTRIB
                                        | EventMask::DELETE_SELF
                                        | EventMask::MOVE_SELF,
                                ) {
                                    should_reload = true;
                                }
                                if event
                                    .mask
                                    .intersects(EventMask::DELETE_SELF | EventMask::MOVE_SELF)
                                {
                                    should_resync = true;
                                }
                            }
                            ConfigWatchSlot::ParentDirectory => {
                                if parent_directory_event_affects_config(
                                    &event,
                                    config_file_name.as_deref(),
                                ) {
                                    should_reload = true;
                                    should_resync = true;
                                }
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

        if should_resync {
            if let Err(error) = self.sync_watches(&config_path) {
                log::warn!("failed to resync config watcher: {:?}", error);
            }
        }

        if should_reload {
            if let Err(error) = reload_from_disk(state, dispatcher, ReloadTrigger::AutomaticWatch) {
                log::warn!("failed to reload config after inotify event: {:#}", error);
            }
        }
    }
}

#[cfg(test)]
impl ConfigWatcher {
    fn watch_slot_count(&self) -> usize {
        self.watches.len()
    }
}

impl Model for ConfigWatcher {
    type Action = ConfigWatcherAction;
    const START: Option<Self::Action> = Some(ConfigWatcherAction::Start);
    const STOP: Option<Self::Action> = Some(ConfigWatcherAction::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            ConfigWatcherAction::Start if !self.stopped => {
                self.dispatch_poll(dispatcher);
            }
            ConfigWatcherAction::Event if !self.stopped => {
                self.on_event(state, dispatcher);
            }
            ConfigWatcherAction::Failed { error } if !self.stopped => {
                self.poll_op_id = None;
                log::warn!("config watcher poll failed: {:?}", error);
                self.dispatch_poll(dispatcher);
            }
            ConfigWatcherAction::Stop => {
                self.stopped = true;
                if let Some(poll_op_id) = self.poll_op_id.take() {
                    dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
                }
                for descriptor in self.watches.keys().cloned().collect::<Vec<_>>() {
                    let _ = self.inotify.watches().remove(descriptor);
                }
                self.watches.clear();
            }
            ConfigWatcherAction::Start
            | ConfigWatcherAction::Event
            | ConfigWatcherAction::Failed { .. } => {}
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
    if changed {
        state.mark_all_workspaces_dirty();
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
        request_outputs(state, state.outputs.iter().map(|(output_id, _)| output_id), dispatcher);
        kick_visual_animation_pump(dispatcher);
    }
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
        request_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
    }
}

pub fn apply_output_settings(state: &mut State, dispatcher: &mut Dispatcher) {
    let changed_outputs = state.apply_output_scales_from_config();
    if changed_outputs.is_empty() {
        return;
    }

    request_outputs(state, changed_outputs.iter().copied(), dispatcher);
    for output_id in changed_outputs {
        dispatcher.dispatch(ClientManagerAction::OutputStateChanged { output_id });
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
            dispatcher.request_shutdown(ShutdownReason::Quit);
        }
        CompositorAction::ReloadConfig => {
            if let Err(error) = reload_from_disk(state, dispatcher, ReloadTrigger::ManualBinding) {
                log::warn!("failed to reload config: {:#}", error);
            }
        }
        CompositorAction::WorkspaceNext => {
            activate_adjacent_workspace(
                state,
                dispatcher,
                crate::visual_animation::WorkspaceTransitionDirection::Next,
            );
        }
        CompositorAction::WorkspacePrevious => {
            activate_adjacent_workspace(
                state,
                dispatcher,
                crate::visual_animation::WorkspaceTransitionDirection::Previous,
            );
        }
        CompositorAction::Close => {
            let Some(root) = require_focused_window(state, "close") else {
                return;
            };
            if !crate::wayland_objects::xdg_surface::request_toplevel_close(state, dispatcher, root)
            {
                log::info!("close ignored; focused window has no toplevel");
            }
        }
        CompositorAction::FocusNext => {
            cycle_focused_workspace(state, dispatcher, FocusCycle::Next);
        }
        CompositorAction::FocusPrevious => {
            cycle_focused_workspace(state, dispatcher, FocusCycle::Previous);
        }
        CompositorAction::ToggleFloating => {
            let Some(root) = require_focused_window(state, "toggle-floating") else {
                return;
            };
            let floating = state
                .managed_root_surface_state(root)
                .is_some_and(|managed| managed.mode == ManagedWindowMode::Floating);
            if state.set_root_surface_floating(root, !floating) {
                finish_root_mode_change(state, dispatcher, root);
            }
        }
        CompositorAction::ToggleFullscreen => {
            let Some(root) = require_focused_window(state, "toggle-fullscreen") else {
                return;
            };
            let fullscreen = state
                .managed_root_surface_state(root)
                .is_some_and(|managed| managed.fullscreen);
            if state.set_root_surface_fullscreen(root, !fullscreen) {
                finish_root_mode_change(state, dispatcher, root);
            }
        }
        CompositorAction::WorkspaceIndex { index } => {
            select_workspace_index(state, dispatcher, index);
        }
        CompositorAction::MoveToWorkspace { index } => {
            move_focused_window_to_workspace(state, dispatcher, index);
        }
    }
}

fn compositor_action_from_file(
    description: &str,
    file: ActionFile,
) -> anyhow::Result<CompositorAction> {
    let ActionFile { action, command, index } = file;
    match action.as_str() {
        "spawn" => {
            reject_index(description, &action, index)?;
            let Some(command) = command.filter(|command| !command.is_empty()) else {
                bail!("binding `{description}` action `spawn` requires `command`");
            };
            Ok(CompositorAction::Spawn { command })
        }
        "quit" => unit_action(description, &action, command, index, CompositorAction::Quit),
        "reload-config" => {
            unit_action(description, &action, command, index, CompositorAction::ReloadConfig)
        }
        "workspace-next" => {
            unit_action(description, &action, command, index, CompositorAction::WorkspaceNext)
        }
        "workspace-previous" => {
            unit_action(description, &action, command, index, CompositorAction::WorkspacePrevious)
        }
        "close" => unit_action(description, &action, command, index, CompositorAction::Close),
        "focus-next" => {
            unit_action(description, &action, command, index, CompositorAction::FocusNext)
        }
        "focus-previous" => {
            unit_action(description, &action, command, index, CompositorAction::FocusPrevious)
        }
        "toggle-floating" => {
            unit_action(description, &action, command, index, CompositorAction::ToggleFloating)
        }
        "toggle-fullscreen" => {
            unit_action(description, &action, command, index, CompositorAction::ToggleFullscreen)
        }
        "workspace-index" => indexed_action(description, &action, command, index, |index| {
            CompositorAction::WorkspaceIndex { index }
        }),
        "move-to-workspace" => indexed_action(description, &action, command, index, |index| {
            CompositorAction::MoveToWorkspace { index }
        }),
        other => bail!("binding `{description}` has unknown action `{other}`"),
    }
}

fn unit_action(
    description: &str,
    action: &str,
    command: Option<String>,
    index: Option<u32>,
    value: CompositorAction,
) -> anyhow::Result<CompositorAction> {
    reject_command(description, action, command)?;
    reject_index(description, action, index)?;
    Ok(value)
}

fn indexed_action(
    description: &str,
    action: &str,
    command: Option<String>,
    index: Option<u32>,
    build: impl FnOnce(u32) -> CompositorAction,
) -> anyhow::Result<CompositorAction> {
    reject_command(description, action, command)?;
    let Some(index) = index else {
        bail!("binding `{description}` action `{action}` requires `index`");
    };
    if index == 0 {
        bail!("binding `{description}` uses workspace index 0; indices are one-based");
    }
    Ok(build(index))
}

fn reject_command(description: &str, action: &str, command: Option<String>) -> anyhow::Result<()> {
    if command.is_some() {
        bail!("binding `{description}` action `{action}` does not take `command`");
    }
    Ok(())
}

fn reject_index(description: &str, action: &str, index: Option<u32>) -> anyhow::Result<()> {
    if index.is_some() {
        bail!("binding `{description}` action `{action}` does not take `index`");
    }
    Ok(())
}

/// The active output's workspace group. Output choice follows [`State::active_output_id`]:
/// the pointer output, then the keyboard focus, then the fallback output.
pub(crate) struct ActiveWorkspaceGroup {
    pub group_id: WorkspaceGroupId,
    pub workspace_id: WorkspaceId,
}

pub(crate) fn active_workspace_group(state: &State) -> Option<ActiveWorkspaceGroup> {
    let output_id = state.active_output_id()?;
    let group_id = state.workspace_group_for_output(output_id)?;
    let workspace_id = state.active_workspace_for_group(group_id)?;
    Some(ActiveWorkspaceGroup { group_id, workspace_id })
}

/// Keyboard focus walked up to a workspace-managed root.
///
/// Sway commands use the seat's focused container. A popup or subsurface here resolves to
/// its managed toplevel. Layer-shell focus and a missing focus have no workspace root.
pub(crate) fn focused_managed_root(state: &State) -> Option<Resolved<WlSurfaceId>> {
    let focus = state.seat.keyboard_focus()?;
    let root = state.root_surface_for_surface(focus.surface)?;
    state
        .managed_root_surface_state(root)
        .is_some()
        .then_some(root)
}

/// One-based index into the active output's workspace list.
///
/// Sway creates a workspace when a binding names one that does not exist. This compositor
/// keeps the output's existing list, so index 0 and an index past that list are absent.
pub(crate) fn workspace_at_index(state: &State, index: u32) -> Option<WorkspaceId> {
    let active = active_workspace_group(state)?;
    let zero_based = usize::try_from(index.checked_sub(1)?).ok()?;
    state
        .workspace_groups
        .try_get(active.group_id)?
        .workspaces
        .get(zero_based)
        .copied()
}

pub(crate) fn require_focused_window(state: &State, action: &str) -> Option<Resolved<WlSurfaceId>> {
    let root = focused_managed_root(state);
    if root.is_none() {
        log::info!("{action} ignored; no focused window");
    }
    root
}

pub(crate) fn require_workspace_index(
    state: &State,
    action: &str,
    index: u32,
) -> Option<WorkspaceId> {
    let workspace_id = workspace_at_index(state, index);
    if workspace_id.is_none() {
        log::info!("{action} ignored; workspace index {index} is unavailable");
    }
    workspace_id
}

enum FocusCycle {
    Next,
    Previous,
}

/// Rotate one stacking band of the focused window's workspace.
///
/// Paint order is tiled windows, back to front, then floating windows. Next moves the
/// backmost window to the front of its band, so repeated next walks tiled windows when
/// any exist and otherwise walks the floating stack. Previous moves the frontmost window
/// to the back of its band, so repeated previous walks floating windows when any exist.
/// A floating raise-to-top would bounce between the top two windows, so the whole band
/// rotates. Tiled windows keep their split slots and move through them.
fn cycle_focused_workspace(state: &mut State, dispatcher: &mut Dispatcher, cycle: FocusCycle) {
    let action = match cycle {
        FocusCycle::Next => "focus-next",
        FocusCycle::Previous => "focus-previous",
    };
    let Some(root) = require_focused_window(state, action) else {
        return;
    };
    let Some(workspace_id) = state.root_surface_workspace(root) else {
        log::info!("{action} ignored; focused window has no workspace");
        return;
    };
    let Some(workspace) = state.workspaces.try_get(workspace_id) else {
        return;
    };
    if workspace.layout.fullscreen.is_some() {
        return;
    }
    let mut tiled = workspace.layout.tiled_scene_order();
    let mut floating = workspace.layout.floating_scene_order();
    if tiled.is_empty() && floating.is_empty() {
        log::info!("{action} ignored; no workspace window");
        return;
    }

    let target = match cycle {
        FocusCycle::Next => {
            if !tiled.is_empty() {
                rotate_back_to_front(&mut tiled);
                tiled[tiled.len() - 1]
            } else {
                rotate_back_to_front(&mut floating);
                floating[floating.len() - 1]
            }
        }
        FocusCycle::Previous => {
            if !floating.is_empty() {
                rotate_front_to_back(&mut floating);
                floating[floating.len() - 1]
            } else {
                rotate_front_to_back(&mut tiled);
                tiled[tiled.len() - 1]
            }
        }
    };

    {
        let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
            return;
        };
        if !workspace.layout.reorder_tiled_scene_order(&tiled)
            || !workspace.layout.set_floating_scene_order(floating)
        {
            return;
        }
        workspace.layout.focus_surface(target);
    }
    let outputs = state.workspace_output_ids(workspace_id);
    if !outputs.is_empty() {
        request_outputs(state, outputs, dispatcher);
    }
    dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    crate::input::set_keyboard_focus(
        state,
        dispatcher,
        Some(crate::input::SurfaceFocus::new(target)),
    );
}

fn rotate_back_to_front(order: &mut Vec<Resolved<WlSurfaceId>>) {
    if order.len() < 2 {
        return;
    }
    let backmost = order.remove(0);
    order.push(backmost);
}

fn rotate_front_to_back(order: &mut Vec<Resolved<WlSurfaceId>>) {
    if order.len() < 2 {
        return;
    }
    let Some(frontmost) = order.pop() else {
        return;
    };
    order.insert(0, frontmost);
}

fn finish_root_mode_change(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root: Resolved<WlSurfaceId>,
) {
    let outputs = state
        .root_surface_workspace(root)
        .map(|workspace_id| state.workspace_output_ids(workspace_id))
        .unwrap_or_default();
    if !outputs.is_empty() {
        request_outputs(state, outputs, dispatcher);
    }
    // A keybinding keeps the focused window. `refresh_scene_focus` would replace keyboard
    // focus with whatever is under the pointer.
    dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
}

fn publish_workspace_group(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    group_id: WorkspaceGroupId,
) {
    let outputs = state.workspace_group_output_ids(group_id);
    publish_workspace_outputs(state, dispatcher, outputs);
}

fn publish_workspace_outputs(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    outputs: impl IntoIterator<Item = crate::state::OutputId>,
) {
    crate::wayland_objects::ext_workspace_manager_v1::publish_workspace_state_changed(
        state, dispatcher, outputs,
    );
    kick_visual_animation_pump(dispatcher);
}

fn select_workspace_index(state: &mut State, dispatcher: &mut Dispatcher, index: u32) {
    let Some(workspace_id) = require_workspace_index(state, "workspace-index", index) else {
        return;
    };
    let Some(group_id) = state
        .workspaces
        .try_get(workspace_id)
        .and_then(|workspace| workspace.group)
    else {
        return;
    };
    // `activate_workspace` is false when the index is already showing. That skips the slide.
    if !state.activate_workspace(workspace_id) {
        return;
    }
    log::info!("workspace index {index} on group {group_id:?}");
    publish_workspace_group(state, dispatcher, group_id);
}

fn move_focused_window_to_workspace(state: &mut State, dispatcher: &mut Dispatcher, index: u32) {
    let Some(root) = require_focused_window(state, "move-to-workspace") else {
        return;
    };
    let Some(workspace_id) = require_workspace_index(state, "move-to-workspace", index) else {
        return;
    };
    let source_workspace = state.root_surface_workspace(root);
    let already_there = source_workspace == Some(workspace_id);
    if !already_there && !state.assign_root_surface_to_workspace(root, workspace_id) {
        log::info!("move-to-workspace ignored; could not move the focused window");
        return;
    }
    let activated = state.activate_workspace(workspace_id);
    if already_there && !activated {
        return;
    }
    // Sway's `move container to workspace` can leave the seat on the source workspace.
    // This binding follows the window, then puts keyboard focus back after publication.
    // Publication refreshes focus from the pointer, so the moved window is focused last.
    log::info!("move window to workspace index {index}");
    let mut outputs = Vec::new();
    if let Some(source_workspace) = source_workspace {
        outputs.extend(state.workspace_output_ids(source_workspace));
    }
    outputs.extend(state.workspace_output_ids(workspace_id));
    publish_workspace_outputs(state, dispatcher, outputs);
    crate::input::set_keyboard_focus(
        state,
        dispatcher,
        Some(crate::input::SurfaceFocus::new(root)),
    );
}

fn activate_adjacent_workspace(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    direction: crate::visual_animation::WorkspaceTransitionDirection,
) {
    let Some(group_id) = state.activate_adjacent_workspace(direction) else {
        log::info!("workspace {direction:?} ignored; this output has only one workspace");
        return;
    };
    log::info!("workspace {direction:?} on group {group_id:?}");
    publish_workspace_group(state, dispatcher, group_id);
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

fn sanitize_non_negative_i32(value: i32) -> i32 {
    value.max(0)
}

fn sanitize_decoration_insets(insets: DecorationInsets) -> DecorationInsets {
    DecorationInsets {
        top: sanitize_non_negative_i32(insets.top),
        right: sanitize_non_negative_i32(insets.right),
        bottom: sanitize_non_negative_i32(insets.bottom),
        left: sanitize_non_negative_i32(insets.left),
    }
}

fn parse_rgba_color(raw: &str) -> anyhow::Result<RgbaColor> {
    fn parse_hex_byte(component: &str, raw: &str) -> anyhow::Result<u8> {
        u8::from_str_radix(component, 16)
            .with_context(|| format!("`{raw}` contains an invalid hex component"))
    }

    fn parse_hex_color(hex: &str, raw: &str) -> anyhow::Result<RgbaColor> {
        match hex.len() {
            6 => Ok(RgbaColor::new(
                parse_hex_byte(&hex[0..2], raw)?,
                parse_hex_byte(&hex[2..4], raw)?,
                parse_hex_byte(&hex[4..6], raw)?,
                0xff,
            )),
            8 => Ok(RgbaColor::new(
                parse_hex_byte(&hex[0..2], raw)?,
                parse_hex_byte(&hex[2..4], raw)?,
                parse_hex_byte(&hex[4..6], raw)?,
                parse_hex_byte(&hex[6..8], raw)?,
            )),
            _ => bail!(
                "expected 6 or 8 hex digits in `{raw}` (supported forms: #RRGGBB, #RRGGBBAA, rgb(...), rgba(...), 0xRRGGBB, 0xRRGGBBAA)"
            ),
        }
    }

    let raw = raw.trim();
    let hex = raw
        .strip_prefix('#')
        .or_else(|| raw.strip_prefix("0x"))
        .or_else(|| raw.strip_prefix("0X"))
        .or_else(|| {
            raw.strip_prefix("rgba(")
                .and_then(|value| value.strip_suffix(')'))
        })
        .or_else(|| {
            raw.strip_prefix("rgb(")
                .and_then(|value| value.strip_suffix(')'))
        })
        .unwrap_or(raw)
        .trim();
    if hex.is_empty() {
        bail!("color must not be empty");
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("`{raw}` is not a valid hex color");
    }
    parse_hex_color(hex, raw)
}

impl fmt::Display for RgbaColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
}

fn parent_directory_event_affects_config(
    event: &inotify::Event<&OsStr>,
    config_file_name: Option<&OsStr>,
) -> bool {
    match event.name {
        None => event
            .mask
            .intersects(EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::ATTRIB),
        Some(name) => {
            let Some(base) = config_file_name else {
                return false;
            };
            if name != base {
                return false;
            }
            event.mask.intersects(
                EventMask::CLOSE_WRITE
                    | EventMask::CREATE
                    | EventMask::DELETE
                    | EventMask::MOVED_FROM
                    | EventMask::MOVED_TO
                    | EventMask::MODIFY
                    | EventMask::ATTRIB,
            )
        }
    }
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
        crate::{
            clients::ClientId,
            input::SurfaceFocus,
            protocol_generated::WlSurfaceId,
            runner::Dispatcher,
            state::{BackendOutputRef, DrmId, OutputId},
            test_support::{
                state_for_tests, state_for_tests_with_config, test_output_with_crtc_name,
            },
            wayland_objects::wl_surface::WlSurface,
        },
    };

    #[test]
    fn parses_toml_binding_with_aliases() {
        let xkb = Xkb::new();
        let parsed: ConfigFile = toml::from_str(
            r##"
            [general]
            cursor_theme = "Adwaita"
            cursor_size = 32
            outer_gap = 12
            inner_gap = 6

            [general.decoration_insets]
            top = 3
            right = 4
            bottom = 5
            left = 6

            [decorations.xdg]
            no_protocol = "server"
            tiled = "server"
            floating = "honor-client"
            fullscreen = "client"
            unset_mode = "client"

            [decorations.border]
            thickness = 4
            active_start_color = "#11223344"
            active_end_color = "#22334455"
            inactive_start_color = "rgba(55667788)"
            inactive_end_color = "#66778899"
            gradient_angle_degrees = 15
            rounding = 7
            rounding_power = 3.5

            [decorations.shadow]
            radius = 18
            offset_x = 2
            offset_y = 9
            color = "#01020380"
            power = 2.5

            [bindings]
            "Super+Enter" = { action = "spawn", command = "alacritty" }
            "Ctrl+Shift+Esc" = { action = "reload-config" }
            "##,
        )
        .unwrap();

        let config = RuntimeConfig::compile(PathBuf::from("config.toml"), parsed, &xkb).unwrap();
        assert_eq!(config.general.cursor_theme, "Adwaita");
        assert_eq!(config.general.cursor_size, 32);
        assert_eq!(config.general.outer_gap, 12);
        assert_eq!(config.general.inner_gap, 6);
        assert_eq!(
            config.general.decoration_insets,
            DecorationInsets { top: 3, right: 4, bottom: 5, left: 6 }
        );
        assert_eq!(config.decorations.xdg.no_protocol, XdgDecorationModePolicy::Server);
        assert_eq!(config.decorations.xdg.tiled, XdgDecorationModePolicy::Server);
        assert_eq!(config.decorations.xdg.floating, XdgDecorationModePolicy::HonorClient);
        assert_eq!(config.decorations.xdg.fullscreen, XdgDecorationModePolicy::Client);
        assert_eq!(config.decorations.xdg.unset_mode, XdgDecorationModePolicy::Client);
        assert_eq!(
            config.decorations.border,
            BorderDecorationConfig {
                thickness: 4,
                active_start_color: RgbaColor::new(0x11, 0x22, 0x33, 0x44),
                active_end_color: RgbaColor::new(0x22, 0x33, 0x44, 0x55),
                inactive_start_color: RgbaColor::new(0x55, 0x66, 0x77, 0x88),
                inactive_end_color: RgbaColor::new(0x66, 0x77, 0x88, 0x99),
                gradient_angle_degrees: 15,
                rounding: 7,
                rounding_power: RoundingPower::sanitize(3.5),
            }
        );
        assert_eq!(
            config.decorations.shadow,
            ShadowDecorationConfig {
                radius: 18,
                offset_x: 2,
                offset_y: 9,
                color: RgbaColor::new(0x01, 0x02, 0x03, 0x80),
                power: ShadowPower::sanitize(2.5),
            }
        );
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
    fn negative_gap_and_inset_values_are_clamped_to_zero() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [general]
            outer_gap = -5
            inner_gap = -7

            [general.decoration_insets]
            top = -1
            right = 2
            bottom = -3
            left = 4

            [decorations.border]
            thickness = -9
            rounding = -5
            rounding_power = 0.0
            "#,
            &xkb,
        )
        .unwrap();

        assert_eq!(config.general.outer_gap, 0);
        assert_eq!(config.general.inner_gap, 0);
        assert_eq!(
            config.general.decoration_insets,
            DecorationInsets { top: 0, right: 2, bottom: 0, left: 4 }
        );
        assert_eq!(config.decorations.border.thickness, 0);
        assert_eq!(config.decorations.border.rounding, 0);
        assert_eq!(config.decorations.border.rounding_power, RoundingPower::DEFAULT);
        assert_eq!(config.decorations.shadow.power, ShadowPower::DEFAULT);
    }

    #[test]
    fn xdg_decoration_policy_defaults_match_tiling_first_behavior() {
        let config = RuntimeConfig::default();

        assert_eq!(config.decorations.xdg.no_protocol, XdgDecorationModePolicy::Client);
        assert_eq!(config.decorations.xdg.tiled, XdgDecorationModePolicy::Server);
        assert_eq!(config.decorations.xdg.floating, XdgDecorationModePolicy::HonorClient);
        assert_eq!(config.decorations.xdg.fullscreen, XdgDecorationModePolicy::Server);
        assert_eq!(config.decorations.xdg.unset_mode, XdgDecorationModePolicy::Server);
        assert_eq!(config.decorations.border.thickness, 0);
        assert_eq!(
            config.decorations.border.active_start_color,
            RgbaColor::new(0x50, 0x78, 0xc8, 0xff)
        );
        assert_eq!(
            config.decorations.border.inactive_start_color,
            RgbaColor::new(0x33, 0x38, 0x44, 0xff)
        );
        assert_eq!(config.decorations.border.rounding, 0);
        assert_eq!(config.decorations.border.rounding_power, RoundingPower::DEFAULT);
    }

    #[test]
    fn parses_supported_color_formats() {
        assert_eq!(parse_rgba_color("#5078c8").unwrap(), RgbaColor::new(0x50, 0x78, 0xc8, 0xff));
        assert_eq!(parse_rgba_color("0x333844ff").unwrap(), RgbaColor::new(0x33, 0x38, 0x44, 0xff));
        assert_eq!(
            parse_rgba_color("rgba(11223344)").unwrap(),
            RgbaColor::new(0x11, 0x22, 0x33, 0x44)
        );
    }

    #[test]
    fn invalid_border_rounding_power_falls_back_to_default() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [decorations.border]
            rounding = 8
            rounding_power = -1.0
            "#,
            &xkb,
        )
        .unwrap();

        assert_eq!(config.decorations.border.rounding_power, RoundingPower::DEFAULT);
    }

    #[test]
    fn invalid_shadow_power_falls_back_to_default() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [decorations.shadow]
            power = 0.0
            "#,
            &xkb,
        )
        .unwrap();

        assert_eq!(config.decorations.shadow.power, ShadowPower::DEFAULT);
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
        let mut state = state_for_tests_with_config(config.clone());
        state.config.path = path.clone();
        let mut dispatcher = Dispatcher::default();

        fs::write(&path, "[bindings\n").unwrap();

        let error = reload_from_disk(&mut state, &mut dispatcher, ReloadTrigger::ManualBinding)
            .unwrap_err();
        assert!(error.to_string().contains("failed to parse"));
        assert_eq!(state.config, config);
    }

    #[test]
    fn reload_dispatches_workspace_state_changed_when_config_changes() {
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let path = temp_dir.to_path_buf().join("config.toml");
        fs::write(
            &path,
            r#"
            [general]
            cursor_theme = "Adwaita"
            outer_gap = 4

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let mut state = state_for_tests_with_config(config);
        state.config.path = path.clone();
        let mut dispatcher = Dispatcher::default();

        fs::write(
            &path,
            r#"
            [general]
            cursor_theme = "Adwaita"
            outer_gap = 16

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let changed =
            reload_from_disk(&mut state, &mut dispatcher, ReloadTrigger::ManualBinding).unwrap();

        assert!(changed);
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| matches!(
            action,
            ClientManagerAction::WorkspaceStateChanged
        )));
    }

    #[test]
    fn reload_marks_all_workspaces_dirty_when_config_changes() {
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let path = temp_dir.to_path_buf().join("config.toml");
        fs::write(
            &path,
            r#"
            [general]
            cursor_theme = "Adwaita"
            outer_gap = 4

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let mut state = state_for_tests_with_config(config);
        state.config.path = path.clone();
        let first_workspace = state.create_workspace(None, Some("first".to_string()));
        let second_workspace = state.create_workspace(None, Some("second".to_string()));
        state
            .workspaces
            .try_get_mut(first_workspace)
            .unwrap()
            .layout
            .dirty = false;
        state
            .workspaces
            .try_get_mut(second_workspace)
            .unwrap()
            .layout
            .dirty = false;
        let mut dispatcher = Dispatcher::default();

        fs::write(
            &path,
            r#"
            [general]
            cursor_theme = "Adwaita"
            outer_gap = 16

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();

        let changed =
            reload_from_disk(&mut state, &mut dispatcher, ReloadTrigger::ManualBinding).unwrap();

        assert!(changed);
        assert!(
            state
                .workspaces
                .try_get(first_workspace)
                .unwrap()
                .layout
                .dirty
        );
        assert!(
            state
                .workspaces
                .try_get(second_workspace)
                .unwrap()
                .layout
                .dirty
        );
    }

    #[test]
    fn config_watcher_missing_file_installs_parent_watch_only() {
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let path = temp_dir.to_path_buf().join("missing.toml");
        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let watcher = ConfigWatcher::new(&config);
        assert_eq!(watcher.watch_slot_count(), 1);
    }

    #[test]
    fn config_watcher_stop_drops_watches_and_ignores_later_events() {
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let path = temp_dir.to_path_buf().join("missing.toml");
        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let mut watcher = ConfigWatcher::new(&config);
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = Dispatcher::default();
        watcher
            .process(&mut state, ConfigWatcherAction::Start, &mut dispatcher)
            .unwrap();
        watcher
            .process(&mut state, ConfigWatcherAction::Stop, &mut dispatcher)
            .unwrap();
        assert_eq!(watcher.watch_slot_count(), 0);
        assert!(dispatcher.test_has_queued_action(|action: &IoUringAction| {
            matches!(action, IoUringAction::PollRemove { .. })
        }));
        watcher
            .process(&mut state, ConfigWatcherAction::Event, &mut dispatcher)
            .unwrap();
        assert_eq!(watcher.watch_slot_count(), 0);
    }

    #[test]
    fn config_watcher_regular_file_installs_parent_and_logical_watches() {
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
        let watcher = ConfigWatcher::new(&config);
        assert_eq!(watcher.watch_slot_count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn config_watcher_symlink_installs_parent_logical_and_resolved_watches() {
        use std::os::unix::fs::symlink;
        let temp_dir = mktemp::Temp::new_dir().unwrap();
        let real = temp_dir.to_path_buf().join("real.toml");
        let logical = temp_dir.to_path_buf().join("config.toml");
        fs::write(
            &real,
            r#"
            [general]
            cursor_theme = "Adwaita"

            [bindings]
            "Super+Return" = { action = "reload-config" }
            "#,
        )
        .unwrap();
        symlink(&real, &logical).unwrap();
        let xkb = Xkb::new();
        let config = RuntimeConfig::load_from_path(&logical, &xkb).unwrap();
        let watcher = ConfigWatcher::new(&config);
        assert_eq!(watcher.watch_slot_count(), 3);
    }

    #[test]
    fn open_duration_defaults_and_can_be_configured() {
        let xkb = Xkb::new();
        let defaults = RuntimeConfig::parse_for_test("", &xkb).unwrap();
        assert_eq!(defaults.animations.window.duration_msec, 160);
        assert_eq!(defaults.animations.window.open_duration_msec, 160);
        assert_eq!(defaults.animations.window.close_duration_msec, 160);
        assert_eq!(defaults.animations.workspace.duration_msec, 250);
        assert!(defaults.animations.workspace.fade);

        let configured = RuntimeConfig::parse_for_test(
            r#"
            [animations.window]
            duration_msec = 40
            open_duration_msec = 0
            close_duration_msec = 0

            [animations.workspace]
            duration_msec = 0
            fade = false
            "#,
            &xkb,
        )
        .unwrap();
        assert_eq!(configured.animations.window.duration_msec, 40);
        assert_eq!(configured.animations.window.open_duration_msec, 0);
        assert_eq!(configured.animations.window.close_duration_msec, 0);
        assert_eq!(configured.animations.workspace.duration_msec, 0);
        assert!(!configured.animations.workspace.fade);
    }

    #[test]
    fn example_config_keeps_the_daily_bindings() {
        let xkb = Xkb::new();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml.example");
        let config = RuntimeConfig::load_from_path(&path, &xkb).unwrap();
        let mut actions: Vec<_> = config
            .bindings
            .iter()
            .map(|binding| (binding.trigger.description.as_str(), binding.action.clone()))
            .collect();
        actions.sort_by_key(|(description, _)| *description);
        assert_eq!(
            actions,
            vec![
                ("Super+1", CompositorAction::WorkspaceIndex { index: 1 }),
                ("Super+2", CompositorAction::WorkspaceIndex { index: 2 }),
                ("Super+3", CompositorAction::WorkspaceIndex { index: 3 }),
                ("Super+4", CompositorAction::WorkspaceIndex { index: 4 }),
                ("Super+Escape", CompositorAction::Quit),
                ("Super+F", CompositorAction::ToggleFullscreen),
                ("Super+Left", CompositorAction::FocusPrevious),
                ("Super+Page_Down", CompositorAction::WorkspacePrevious),
                ("Super+Page_Up", CompositorAction::WorkspaceNext),
                ("Super+Q", CompositorAction::Close),
                ("Super+Return", CompositorAction::Spawn { command: "alacritty".to_string() }),
                ("Super+Right", CompositorAction::FocusNext),
                ("Super+Shift+1", CompositorAction::MoveToWorkspace { index: 1 }),
                ("Super+Shift+2", CompositorAction::MoveToWorkspace { index: 2 }),
                ("Super+Shift+3", CompositorAction::MoveToWorkspace { index: 3 }),
                ("Super+Shift+4", CompositorAction::MoveToWorkspace { index: 4 }),
                ("Super+Shift+C", CompositorAction::ReloadConfig),
                ("Super+Space", CompositorAction::ToggleFloating),
            ]
        );
        let mouse: Vec<_> = config
            .mouse_bindings
            .iter()
            .map(|binding| (binding.trigger.description.as_str(), binding.action))
            .collect();
        assert_eq!(
            mouse,
            vec![
                ("Ctrl+mouse:left", MouseBindingAction::MoveWindow),
                ("Ctrl+mouse:right", MouseBindingAction::ResizeWindow),
            ]
        );
    }

    #[test]
    fn parses_window_and_indexed_workspace_actions() {
        let xkb = Xkb::new();
        let config = RuntimeConfig::parse_for_test(
            r#"
            [bindings]
            "Super+Q" = { action = "close" }
            "Super+Left" = { action = "focus-previous" }
            "Super+Right" = { action = "focus-next" }
            "Super+Space" = { action = "toggle-floating" }
            "Super+F" = { action = "toggle-fullscreen" }
            "Super+1" = { action = "workspace-index", index = 1 }
            "Super+Shift+1" = { action = "move-to-workspace", index = 4 }
            "#,
            &xkb,
        )
        .unwrap();

        let actions: Vec<_> = config
            .bindings
            .into_iter()
            .map(|binding| binding.action)
            .collect();
        assert_eq!(
            actions,
            vec![
                CompositorAction::WorkspaceIndex { index: 1 },
                CompositorAction::ToggleFullscreen,
                CompositorAction::FocusPrevious,
                CompositorAction::Close,
                CompositorAction::FocusNext,
                CompositorAction::MoveToWorkspace { index: 4 },
                CompositorAction::ToggleFloating,
            ]
        );
    }

    #[test]
    fn rejects_a_zero_workspace_index_at_load() {
        let xkb = Xkb::new();
        for action in ["workspace-index", "move-to-workspace"] {
            let error = RuntimeConfig::parse_for_test(
                &format!(
                    r#"
                    [bindings]
                    "Super+1" = {{ action = "{action}", index = 0 }}
                    "#
                ),
                &xkb,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("one-based"),
                "{action} accepted index 0: {error:#}"
            );
        }
    }

    #[test]
    fn rejects_fields_that_do_not_belong_to_the_action() {
        let xkb = Xkb::new();
        let wrong_index = RuntimeConfig::parse_for_test(
            r#"
            [bindings]
            "Super+Q" = { action = "close", index = 1 }
            "#,
            &xkb,
        )
        .unwrap_err();
        assert!(wrong_index.to_string().contains("does not take `index`"));

        let unknown = RuntimeConfig::parse_for_test(
            r#"
            [bindings]
            "Super+Q" = { action = "close", extra = 1 }
            "#,
            &xkb,
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));
    }

    fn insert_managed_root(state: &mut State) -> (OutputId, Resolved<WlSurfaceId>) {
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(41u32), ClientId::from_raw(1), 6));
        let surface = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(41u32), object_id);
        state.register_root_surface(surface);
        assert!(state.managed_root_surface_state(surface).is_some());
        (output, surface)
    }

    #[test]
    fn focused_window_actions_need_a_managed_root() {
        let mut state = state_for_tests();
        assert!(focused_managed_root(&state).is_none());

        let (_output, surface) = insert_managed_root(&mut state);
        assert!(focused_managed_root(&state).is_none());

        state.seat.keyboard_focus = Some(SurfaceFocus::new(surface));
        assert_eq!(focused_managed_root(&state), Some(surface));

        state.managed_root_surfaces.remove(&surface);
        assert!(focused_managed_root(&state).is_none());
    }

    #[test]
    fn workspace_index_uses_the_active_output_and_ignores_invalid_slots() {
        let mut state = state_for_tests();
        assert!(workspace_at_index(&state, 1).is_none());

        let (output, _surface) = insert_managed_root(&mut state);
        let group_id = state.workspace_group_for_output(output).unwrap();
        let workspaces = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .workspaces
            .clone();
        assert_eq!(workspaces.len(), 4);
        assert_eq!(workspace_at_index(&state, 1), Some(workspaces[0]));
        assert_eq!(workspace_at_index(&state, 4), Some(workspaces[3]));
        assert!(workspace_at_index(&state, 0).is_none());
        assert!(workspace_at_index(&state, 5).is_none());

        let before = state.active_workspace_for_output(output);
        execute_action(
            &mut state,
            &mut Dispatcher::default(),
            CompositorAction::WorkspaceIndex { index: 5 },
        );
        assert_eq!(state.active_workspace_for_output(output), before);
    }

    fn focus_managed_root(state: &mut State) -> Resolved<WlSurfaceId> {
        let (_output, surface) = insert_managed_root(state);
        state.seat.keyboard_focus = Some(SurfaceFocus::new(surface));
        surface
    }

    #[test]
    fn close_without_a_toplevel_is_a_no_op() {
        let mut state = state_for_tests();
        let surface = focus_managed_root(&mut state);
        let mut dispatcher = Dispatcher::default();
        execute_action(&mut state, &mut dispatcher, CompositorAction::Close);
        assert!(!dispatcher.test_has_queued_action::<crate::clients::ClientAction>(|action| {
            matches!(action, crate::clients::ClientAction::RequestToplevelClose { .. })
        }));
        assert!(state.managed_root_surface_state(surface).is_some());
    }

    #[test]
    fn toggle_floating_and_fullscreen_flip_and_restore() {
        let mut state = state_for_tests();
        let surface = focus_managed_root(&mut state);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::ToggleFloating);
        assert_eq!(
            state.managed_root_surface_state(surface).unwrap().mode,
            crate::state::ManagedWindowMode::Floating
        );
        execute_action(&mut state, &mut dispatcher, CompositorAction::ToggleFloating);
        assert_eq!(
            state.managed_root_surface_state(surface).unwrap().mode,
            crate::state::ManagedWindowMode::Tiled
        );

        execute_action(&mut state, &mut dispatcher, CompositorAction::ToggleFullscreen);
        assert!(
            state
                .managed_root_surface_state(surface)
                .unwrap()
                .fullscreen
        );
        execute_action(&mut state, &mut dispatcher, CompositorAction::ToggleFullscreen);
        assert!(
            !state
                .managed_root_surface_state(surface)
                .unwrap()
                .fullscreen
        );
    }

    fn add_root(state: &mut State, raw: u32) -> Resolved<WlSurfaceId> {
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(raw), ClientId::from_raw(1), 6));
        let surface = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(raw), object_id);
        state.register_root_surface(surface);
        surface
    }

    fn layout_mut<'a>(
        state: &'a mut State,
        surface: Resolved<WlSurfaceId>,
    ) -> &'a mut crate::tiling::WorkspaceLayout {
        let workspace_id = state.root_surface_workspace(surface).unwrap();
        &mut state.workspaces.try_get_mut(workspace_id).unwrap().layout
    }

    fn place_tiled(state: &mut State, surface: Resolved<WlSurfaceId>) {
        let layout = layout_mut(state, surface);
        if !layout.is_tiled_surface(surface) {
            assert!(layout.insert_tiled(surface));
        }
    }

    fn place_floating(state: &mut State, surface: Resolved<WlSurfaceId>) {
        {
            let layout = layout_mut(state, surface);
            if !layout.contains_surface(surface) {
                assert!(layout.insert_tiled(surface));
            }
            if !layout.is_floating_surface(surface) {
                assert!(layout.set_floating(surface, true));
            }
        }
        state.managed_root_surfaces.get_mut(&surface).unwrap().mode =
            crate::state::ManagedWindowMode::Floating;
    }

    fn scene_orders(
        state: &State,
        surface: Resolved<WlSurfaceId>,
    ) -> (Vec<Resolved<WlSurfaceId>>, Vec<Resolved<WlSurfaceId>>) {
        let workspace_id = state.root_surface_workspace(surface).unwrap();
        let layout = &state.workspaces.try_get(workspace_id).unwrap().layout;
        (layout.tiled_scene_order(), layout.floating_scene_order())
    }

    fn keyboard_surface(state: &State) -> Option<Resolved<WlSurfaceId>> {
        state.seat.keyboard_focus.map(|focus| focus.surface)
    }

    #[test]
    fn focus_cycle_on_one_window_stays_put() {
        let mut state = state_for_tests();
        let surface = focus_managed_root(&mut state);
        place_tiled(&mut state, surface);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(keyboard_surface(&state), Some(surface));
        assert_eq!(scene_orders(&state, surface).0, vec![surface]);

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(keyboard_surface(&state), Some(surface));
        assert_eq!(scene_orders(&state, surface).0, vec![surface]);
    }

    #[test]
    fn focus_cycle_rotates_floating_windows_without_bouncing() {
        let mut state = state_for_tests();
        let first = focus_managed_root(&mut state);
        let second = add_root(&mut state, 42);
        let third = add_root(&mut state, 43);
        let fourth = add_root(&mut state, 44);
        for surface in [first, second, third, fourth] {
            place_floating(&mut state, surface);
        }
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, first).1, vec![second, third, fourth, first]);
        assert_eq!(keyboard_surface(&state), Some(first));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, first).1, vec![third, fourth, first, second]);
        assert_eq!(keyboard_surface(&state), Some(second));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, first).1, vec![fourth, first, second, third]);
        assert_eq!(keyboard_surface(&state), Some(third));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, first).1, vec![first, second, third, fourth]);
        assert_eq!(keyboard_surface(&state), Some(fourth));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, first).1, vec![fourth, first, second, third]);
        assert_eq!(keyboard_surface(&state), Some(third));
    }

    #[test]
    fn focus_cycle_two_windows_swaps_in_either_direction() {
        let mut state = state_for_tests();
        let back = focus_managed_root(&mut state);
        let front = add_root(&mut state, 42);
        place_floating(&mut state, back);
        place_floating(&mut state, front);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, back).1, vec![front, back]);
        assert_eq!(keyboard_surface(&state), Some(back));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, back).1, vec![back, front]);
        assert_eq!(keyboard_surface(&state), Some(front));
    }

    #[test]
    fn focus_cycle_keeps_tiled_and_floating_in_separate_bands() {
        let mut state = state_for_tests();
        let tiled_back = focus_managed_root(&mut state);
        let tiled_front = add_root(&mut state, 42);
        let floating_back = add_root(&mut state, 43);
        let floating_front = add_root(&mut state, 44);
        place_tiled(&mut state, tiled_back);
        place_tiled(&mut state, tiled_front);
        place_floating(&mut state, floating_back);
        place_floating(&mut state, floating_front);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, tiled_back).0, vec![tiled_front, tiled_back]);
        assert_eq!(scene_orders(&state, tiled_back).1, vec![floating_back, floating_front]);
        assert_eq!(keyboard_surface(&state), Some(tiled_back));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusNext);
        assert_eq!(scene_orders(&state, tiled_back).0, vec![tiled_back, tiled_front]);
        assert_eq!(keyboard_surface(&state), Some(tiled_front));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, tiled_back).0, vec![tiled_back, tiled_front]);
        assert_eq!(scene_orders(&state, tiled_back).1, vec![floating_front, floating_back]);
        assert_eq!(keyboard_surface(&state), Some(floating_back));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, tiled_back).1, vec![floating_back, floating_front]);
        assert_eq!(keyboard_surface(&state), Some(floating_front));
    }

    #[test]
    fn focus_cycle_rotates_tiled_windows_when_nothing_floats() {
        let mut state = state_for_tests();
        let first = focus_managed_root(&mut state);
        let second = add_root(&mut state, 42);
        let third = add_root(&mut state, 43);
        place_tiled(&mut state, first);
        place_tiled(&mut state, second);
        place_tiled(&mut state, third);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, first).0, vec![third, first, second]);
        assert_eq!(keyboard_surface(&state), Some(second));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, first).0, vec![second, third, first]);
        assert_eq!(keyboard_surface(&state), Some(first));

        execute_action(&mut state, &mut dispatcher, CompositorAction::FocusPrevious);
        assert_eq!(scene_orders(&state, first).0, vec![first, second, third]);
        assert_eq!(keyboard_surface(&state), Some(third));
    }

    fn group_workspaces(state: &State, output: OutputId) -> Vec<crate::state::WorkspaceId> {
        let group_id = state.workspace_group_for_output(output).unwrap();
        state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .workspaces
            .clone()
    }

    fn group_transition(
        state: &State,
        output: OutputId,
    ) -> Option<crate::visual_animation::WorkspaceGroupTransition> {
        let group_id = state.workspace_group_for_output(output).unwrap();
        state.workspace_groups.try_get(group_id).unwrap().transition
    }

    fn output_frame_requested(dispatcher: &Dispatcher, output: OutputId) -> bool {
        dispatcher.test_has_queued_action_for(
            crate::scheduler::output_scheduler::output_scheduler_id(output),
            |action: &crate::scheduler::output_scheduler::OutputSchedulerAction| {
                matches!(
                    action,
                    crate::scheduler::output_scheduler::OutputSchedulerAction::RequestFrame { .. }
                )
            },
        )
    }

    #[test]
    fn workspace_index_slides_within_the_active_output_group() {
        let mut state = state_for_tests();
        state.config.animations.workspace.duration_msec = 250;
        let (left, _surface) = insert_managed_root(&mut state);
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let workspaces = group_workspaces(&state, left);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::WorkspaceIndex { index: 1 });
        assert_eq!(state.active_workspace_for_output(left), Some(workspaces[0]));
        assert!(group_transition(&state, left).is_none());
        assert!(!output_frame_requested(&dispatcher, left));

        execute_action(&mut state, &mut dispatcher, CompositorAction::WorkspaceIndex { index: 0 });
        assert_eq!(state.active_workspace_for_output(left), Some(workspaces[0]));

        for index in 2..=4 {
            execute_action(&mut state, &mut dispatcher, CompositorAction::WorkspaceIndex { index });
            let slot = (index - 1) as usize;
            assert_eq!(state.active_workspace_for_output(left), Some(workspaces[slot]));
            let transition = group_transition(&state, left).unwrap();
            assert_eq!(
                transition.direction(),
                crate::visual_animation::WorkspaceTransitionDirection::Next
            );
            assert_eq!(transition.to_workspace(), workspaces[slot]);
        }
        let stayed = group_transition(&state, left).unwrap();
        execute_action(&mut state, &mut dispatcher, CompositorAction::WorkspaceIndex { index: 4 });
        let still = group_transition(&state, left).unwrap();
        assert_eq!(still.from_workspace(), stayed.from_workspace());
        assert_eq!(still.to_workspace(), stayed.to_workspace());
        assert_eq!(still.direction(), stayed.direction());

        execute_action(&mut state, &mut dispatcher, CompositorAction::WorkspaceIndex { index: 1 });
        let back = group_transition(&state, left).unwrap();
        assert_eq!(
            back.direction(),
            crate::visual_animation::WorkspaceTransitionDirection::Previous
        );
        assert_eq!(back.from_workspace(), workspaces[3]);
        assert_eq!(back.to_workspace(), workspaces[0]);
        assert!(output_frame_requested(&dispatcher, left));
        assert!(!output_frame_requested(&dispatcher, right));
    }

    #[test]
    fn move_to_workspace_follows_the_window_and_keeps_its_mode() {
        let mut state = state_for_tests();
        state.config.animations.workspace.duration_msec = 250;
        let surface = focus_managed_root(&mut state);
        assert!(state.set_root_surface_floating(surface, true));
        assert!(state.set_root_surface_fullscreen(surface, true));
        let source = state.root_surface_workspace(surface).unwrap();
        state
            .workspaces
            .try_get_mut(source)
            .unwrap()
            .layout
            .set_fullscreen(Some(surface));
        let output = state.workspace_output_id(source).unwrap();
        let workspaces = group_workspaces(&state, output);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::MoveToWorkspace { index: 4 });
        assert_eq!(state.root_surface_workspace(surface), Some(workspaces[3]));
        assert_eq!(state.active_workspace_for_output(output), Some(workspaces[3]));
        assert_eq!(keyboard_surface(&state), Some(surface));
        let managed = state.managed_root_surface_state(surface).unwrap();
        assert_eq!(managed.mode, crate::state::ManagedWindowMode::Floating);
        assert!(managed.fullscreen);
        assert!(state
            .workspaces
            .try_get(source)
            .unwrap()
            .layout
            .fullscreen
            .is_none());
        let transition = group_transition(&state, output).unwrap();
        assert_eq!(
            transition.direction(),
            crate::visual_animation::WorkspaceTransitionDirection::Next
        );
        assert_eq!(transition.from_workspace(), workspaces[0]);
        assert_eq!(transition.to_workspace(), workspaces[3]);

        execute_action(&mut state, &mut dispatcher, CompositorAction::MoveToWorkspace { index: 4 });
        assert_eq!(state.root_surface_workspace(surface), Some(workspaces[3]));
        assert_eq!(state.active_workspace_for_output(output), Some(workspaces[3]));
        assert_eq!(keyboard_surface(&state), Some(surface));
        let again = group_transition(&state, output).unwrap();
        assert_eq!(again.from_workspace(), transition.from_workspace());
        assert_eq!(again.to_workspace(), transition.to_workspace());
    }

    #[test]
    fn move_to_workspace_publishes_the_source_and_destination_outputs() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(51u32), ClientId::from_raw(1), 6));
        let surface = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(51u32), object_id);
        state.register_root_surface(surface);
        state.seat.keyboard_focus = Some(SurfaceFocus::new(surface));
        let source = state.root_surface_workspace(surface).unwrap();
        state.seat.pointer_pos = (150.0, 10.0);
        let right_workspaces = group_workspaces(&state, right);
        let mut dispatcher = Dispatcher::default();

        execute_action(&mut state, &mut dispatcher, CompositorAction::MoveToWorkspace { index: 1 });
        assert_eq!(state.root_surface_workspace(surface), Some(right_workspaces[0]));
        assert_eq!(state.active_workspace_for_output(left), Some(source));
        assert_eq!(state.active_workspace_for_output(right), Some(right_workspaces[0]));
        assert_eq!(keyboard_surface(&state), Some(surface));
        assert!(output_frame_requested(&dispatcher, left));
        assert!(output_frame_requested(&dispatcher, right));
    }
}
