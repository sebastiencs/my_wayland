//! Inheritable animation nodes and named Bézier curves.
//!
//! Speed uses Hyprland's unit: the duration is `speed * 100` milliseconds. The shipped tree matches
//! Hyprland's example config. A field present in that base stays in place until the user's config
//! replaces that field. A field present in neither layer inherits the resolved parent, which is how
//! `windowsMove` follows `windows`.

use std::{collections::BTreeMap, time::Duration};

use {
    anyhow::{anyhow, bail, Context},
    serde::Deserialize,
};

use crate::visual_animation::{CubicBezier, Easing};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum AnimationNode {
    Global,
    Windows,
    WindowsIn,
    WindowsOut,
    WindowsMove,
    Fade,
    FadeIn,
    FadeOut,
    Workspaces,
    WorkspacesIn,
    WorkspacesOut,
}

impl AnimationNode {
    pub const ALL: [Self; 11] = [
        Self::Global,
        Self::Windows,
        Self::WindowsIn,
        Self::WindowsOut,
        Self::WindowsMove,
        Self::Fade,
        Self::FadeIn,
        Self::FadeOut,
        Self::Workspaces,
        Self::WorkspacesIn,
        Self::WorkspacesOut,
    ];

    fn parent(self) -> Option<Self> {
        match self {
            Self::Global => None,
            Self::Windows | Self::Fade | Self::Workspaces => Some(Self::Global),
            Self::WindowsIn | Self::WindowsOut | Self::WindowsMove => Some(Self::Windows),
            Self::FadeIn | Self::FadeOut => Some(Self::Fade),
            Self::WorkspacesIn | Self::WorkspacesOut => Some(Self::Workspaces),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Windows => "windows",
            Self::WindowsIn => "windowsIn",
            Self::WindowsOut => "windowsOut",
            Self::WindowsMove => "windowsMove",
            Self::Fade => "fade",
            Self::FadeIn => "fadeIn",
            Self::FadeOut => "fadeOut",
            Self::Workspaces => "workspaces",
            Self::WorkspacesIn => "workspacesIn",
            Self::WorkspacesOut => "workspacesOut",
        }
    }

    fn family(self) -> StyleFamily {
        match self {
            Self::Windows | Self::WindowsIn | Self::WindowsOut | Self::WindowsMove => {
                StyleFamily::Window
            }
            Self::Workspaces | Self::WorkspacesIn | Self::WorkspacesOut => StyleFamily::Workspace,
            Self::Global | Self::Fade | Self::FadeIn | Self::FadeOut => StyleFamily::Plain,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StyleFamily {
    Plain,
    Window,
    Workspace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlideEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationStyle {
    None,
    Popin { percent: u8 },
    Slide { edge: Option<SlideEdge> },
    Gnomed,
    Fade,
    SlideFade,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedAnimation {
    enabled: bool,
    speed: f64,
    bezier: CubicBezier,
    style: AnimationStyle,
}

impl PartialEq for ResolvedAnimation {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.speed.to_bits() == other.speed.to_bits()
            && self.bezier == other.bezier
            && self.style == other.style
    }
}

impl Eq for ResolvedAnimation {}

impl ResolvedAnimation {
    pub fn enabled(self) -> bool {
        self.enabled
    }

    pub fn speed(self) -> f64 {
        self.speed
    }

    pub fn duration(self) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }
        let millis = (self.speed * 100.0).round();
        if !(1.0..u64::MAX as f64).contains(&millis) {
            return Duration::ZERO;
        }
        Duration::from_millis(millis as u64)
    }

    pub fn duration_msec(self) -> u32 {
        u32::try_from(self.duration().as_millis()).unwrap_or(u32::MAX)
    }

    pub fn easing(self) -> Easing {
        Easing::Cubic(self.bezier)
    }

    pub fn style(self) -> AnimationStyle {
        self.style
    }

    pub fn fades(self) -> bool {
        matches!(self.style, AnimationStyle::Fade | AnimationStyle::SlideFade)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationsConfig {
    nodes: BTreeMap<AnimationNode, ResolvedAnimation>,
}

impl AnimationsConfig {
    pub fn node(&self, node: AnimationNode) -> ResolvedAnimation {
        self.nodes[&node]
    }

    pub fn focus(&self) -> ResolvedAnimation {
        self.node(AnimationNode::Fade)
    }

    pub fn opening(&self) -> ResolvedAnimation {
        self.node(AnimationNode::WindowsIn)
    }

    pub fn closing(&self) -> ResolvedAnimation {
        self.node(AnimationNode::WindowsOut)
    }

    pub fn moving(&self) -> ResolvedAnimation {
        self.node(AnimationNode::WindowsMove)
    }

    pub fn workspace(&self) -> ResolvedAnimation {
        self.node(AnimationNode::Workspaces)
    }

    pub fn set_duration_msec(&mut self, node: AnimationNode, msec: u32) {
        let resolved = self.nodes.get_mut(&node).expect("animation node");
        if msec == 0 {
            resolved.enabled = false;
            return;
        }
        resolved.enabled = true;
        resolved.speed = f64::from(msec) / 100.0;
    }

    pub fn set_focus_duration_msec(&mut self, msec: u32) {
        self.set_duration_msec(AnimationNode::Fade, msec);
    }

    pub fn set_open_duration_msec(&mut self, msec: u32) {
        self.set_duration_msec(AnimationNode::WindowsIn, msec);
    }

    pub fn set_close_duration_msec(&mut self, msec: u32) {
        self.set_duration_msec(AnimationNode::WindowsOut, msec);
    }

    pub fn set_workspace_duration_msec(&mut self, msec: u32) {
        self.set_duration_msec(AnimationNode::Workspaces, msec);
    }

    pub fn set_linear(&mut self, node: AnimationNode) {
        let resolved = self.nodes.get_mut(&node).expect("animation node");
        resolved.bezier = CubicBezier::linear();
    }

    pub fn set_style(&mut self, node: AnimationNode, style: AnimationStyle) {
        self.nodes.get_mut(&node).expect("animation node").style = style;
    }

    pub fn set_workspace_fade(&mut self, fade: bool) {
        let resolved = self
            .nodes
            .get_mut(&AnimationNode::Workspaces)
            .expect("workspaces");
        resolved.style =
            if fade { AnimationStyle::Fade } else { AnimationStyle::Slide { edge: None } };
    }
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        compile(AnimationsConfigFile::default()).expect("default animation tree")
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AnimationsConfigFile {
    bezier: BTreeMap<String, BezierFile>,
    global: Option<AnimationNodeFile>,
    windows: Option<AnimationNodeFile>,
    #[serde(rename = "windowsIn")]
    windows_in: Option<AnimationNodeFile>,
    #[serde(rename = "windowsOut")]
    windows_out: Option<AnimationNodeFile>,
    #[serde(rename = "windowsMove")]
    windows_move: Option<AnimationNodeFile>,
    fade: Option<AnimationNodeFile>,
    #[serde(rename = "fadeIn")]
    fade_in: Option<AnimationNodeFile>,
    #[serde(rename = "fadeOut")]
    fade_out: Option<AnimationNodeFile>,
    workspaces: Option<AnimationNodeFile>,
    #[serde(rename = "workspacesIn")]
    workspaces_in: Option<AnimationNodeFile>,
    #[serde(rename = "workspacesOut")]
    workspaces_out: Option<AnimationNodeFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BezierFile {
    points: Option<[f64; 4]>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AnimationNodeFile {
    enabled: Option<bool>,
    speed: Option<f64>,
    bezier: Option<String>,
    style: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct NodePatch {
    enabled: Option<bool>,
    speed: Option<f64>,
    bezier: Option<String>,
    style: Option<String>,
}

pub(crate) fn compile(file: AnimationsConfigFile) -> anyhow::Result<AnimationsConfig> {
    let curves = bezier_library(&file)?;
    let mut patches = example_patches();
    for (node, patch) in user_patches(&file) {
        let base = patches.entry(node).or_default();
        overlay(base, patch);
    }
    resolve(patches, &curves)
}

fn bezier_library(file: &AnimationsConfigFile) -> anyhow::Result<BTreeMap<String, CubicBezier>> {
    let mut curves = builtin_beziers();
    for (name, bezier) in &file.bezier {
        if name.is_empty() {
            bail!("animation bezier name is empty");
        }
        let Some(points) = bezier.points else {
            bail!("animations.bezier.{name} needs four control points");
        };
        curves.insert(name.clone(), validate_bezier(name, points)?);
    }
    Ok(curves)
}

fn builtin_beziers() -> BTreeMap<String, CubicBezier> {
    BTreeMap::from([
        ("default".to_string(), CubicBezier::new(0.0, 0.75, 0.15, 1.0)),
        ("linear".to_string(), CubicBezier::linear()),
        ("easeOutQuint".to_string(), CubicBezier::new(0.23, 1.0, 0.32, 1.0)),
        ("easeInOutCubic".to_string(), CubicBezier::new(0.65, 0.05, 0.36, 1.0)),
        ("almostLinear".to_string(), CubicBezier::new(0.5, 0.5, 0.75, 1.0)),
        ("quick".to_string(), CubicBezier::new(0.15, 0.0, 0.1, 1.0)),
    ])
}

fn validate_bezier(name: &str, points: [f64; 4]) -> anyhow::Result<CubicBezier> {
    let [x1, y1, x2, y2] = points;
    if ![x1, y1, x2, y2].into_iter().all(f64::is_finite) {
        bail!("animations.bezier.{name} has a non-finite control point");
    }
    if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
        bail!("animations.bezier.{name} control x values must be within 0..=1");
    }
    Ok(CubicBezier::new(x1, y1, x2, y2))
}

fn example_patches() -> BTreeMap<AnimationNode, NodePatch> {
    BTreeMap::from([
        (AnimationNode::Global, patch(true, 10.0, "default", "")),
        (AnimationNode::Windows, patch(true, 4.79, "easeOutQuint", "")),
        (AnimationNode::WindowsIn, patch(true, 4.1, "easeOutQuint", "popin 87%")),
        (AnimationNode::WindowsOut, patch(true, 1.49, "linear", "popin 87%")),
        (AnimationNode::Fade, patch(true, 3.03, "quick", "")),
        (AnimationNode::FadeIn, patch(true, 1.73, "almostLinear", "")),
        (AnimationNode::FadeOut, patch(true, 1.46, "almostLinear", "")),
        (AnimationNode::Workspaces, patch(true, 1.94, "almostLinear", "fade")),
        (AnimationNode::WorkspacesIn, patch(true, 1.21, "almostLinear", "fade")),
        (AnimationNode::WorkspacesOut, patch(true, 1.94, "almostLinear", "fade")),
    ])
}

fn patch(enabled: bool, speed: f64, bezier: &str, style: &str) -> NodePatch {
    NodePatch {
        enabled: Some(enabled),
        speed: Some(speed),
        bezier: Some(bezier.to_string()),
        style: Some(style.to_string()),
    }
}

fn user_patches(file: &AnimationsConfigFile) -> BTreeMap<AnimationNode, NodePatch> {
    let entries = [
        (AnimationNode::Global, &file.global),
        (AnimationNode::Windows, &file.windows),
        (AnimationNode::WindowsIn, &file.windows_in),
        (AnimationNode::WindowsOut, &file.windows_out),
        (AnimationNode::WindowsMove, &file.windows_move),
        (AnimationNode::Fade, &file.fade),
        (AnimationNode::FadeIn, &file.fade_in),
        (AnimationNode::FadeOut, &file.fade_out),
        (AnimationNode::Workspaces, &file.workspaces),
        (AnimationNode::WorkspacesIn, &file.workspaces_in),
        (AnimationNode::WorkspacesOut, &file.workspaces_out),
    ];
    entries
        .into_iter()
        .filter_map(|(node, file)| file.as_ref().map(|file| (node, patch_from_file(file))))
        .collect()
}

fn patch_from_file(file: &AnimationNodeFile) -> NodePatch {
    NodePatch {
        enabled: file.enabled,
        speed: file.speed,
        bezier: file.bezier.clone(),
        style: file.style.clone(),
    }
}

fn overlay(base: &mut NodePatch, user: NodePatch) {
    if user.enabled.is_some() {
        base.enabled = user.enabled;
    }
    if user.speed.is_some() {
        base.speed = user.speed;
    }
    if user.bezier.is_some() {
        base.bezier = user.bezier;
    }
    if user.style.is_some() {
        base.style = user.style;
    }
}

fn resolve(
    patches: BTreeMap<AnimationNode, NodePatch>,
    curves: &BTreeMap<String, CubicBezier>,
) -> anyhow::Result<AnimationsConfig> {
    let mut nodes = BTreeMap::new();
    for node in AnimationNode::ALL {
        let parent = node.parent().map(|parent| nodes[&parent]);
        nodes.insert(node, resolve_node(node, patches.get(&node), parent, curves)?);
    }
    Ok(AnimationsConfig { nodes })
}

fn resolve_node(
    node: AnimationNode,
    patch: Option<&NodePatch>,
    parent: Option<ResolvedAnimation>,
    curves: &BTreeMap<String, CubicBezier>,
) -> anyhow::Result<ResolvedAnimation> {
    let seed = parent.unwrap_or(ResolvedAnimation {
        enabled: true,
        speed: 8.0,
        bezier: curves["default"],
        style: AnimationStyle::None,
    });
    let patch = patch.cloned().unwrap_or_default();
    if let Some(speed) = patch.speed {
        if !speed.is_finite() || speed <= 0.0 {
            bail!("animations.{}.speed must be greater than 0", node.name());
        }
    }
    let bezier = match patch.bezier {
        Some(name) => *curves.get(&name).ok_or_else(|| {
            anyhow!("animations.{} references unknown bezier `{name}`", node.name())
        })?,
        None => seed.bezier,
    };
    let style = match patch.style {
        Some(style) => parse_style(node, &style)?,
        None => seed.style,
    };
    Ok(ResolvedAnimation {
        enabled: patch.enabled.unwrap_or(seed.enabled),
        speed: patch.speed.unwrap_or(seed.speed),
        bezier,
        style,
    })
}

fn parse_style(node: AnimationNode, style: &str) -> anyhow::Result<AnimationStyle> {
    let style = style.trim();
    if style.is_empty() {
        return Ok(AnimationStyle::None);
    }
    let mut words = style.split_whitespace();
    let kind = words.next().unwrap();
    let argument = words.collect::<Vec<_>>().join(" ");
    let parsed = match (node.family(), kind) {
        (StyleFamily::Window, "popin") => {
            AnimationStyle::Popin { percent: parse_percent(node, &argument)? }
        }
        (StyleFamily::Window, "slide") => {
            AnimationStyle::Slide { edge: parse_edge(node, &argument)? }
        }
        (StyleFamily::Window, "gnomed") if argument.is_empty() => AnimationStyle::Gnomed,
        (StyleFamily::Workspace, "fade") if argument.is_empty() => AnimationStyle::Fade,
        (StyleFamily::Workspace, "slide") if argument.is_empty() => {
            AnimationStyle::Slide { edge: None }
        }
        (StyleFamily::Workspace, "slidefade") if argument.is_empty() => AnimationStyle::SlideFade,
        _ => bail!("animations.{} has unsupported style `{style}`", node.name()),
    };
    Ok(parsed)
}

fn parse_percent(node: AnimationNode, argument: &str) -> anyhow::Result<u8> {
    if argument.is_empty() {
        return Ok(0);
    }
    let number = argument.strip_suffix('%').unwrap_or(argument);
    let percent = number.parse::<u32>().with_context(|| {
        format!("animations.{} popin percent `{argument}` is invalid", node.name())
    })?;
    u8::try_from(percent)
        .ok()
        .filter(|percent| *percent <= 100)
        .ok_or_else(|| anyhow!("animations.{} popin percent must be within 0..=100", node.name()))
}

fn parse_edge(node: AnimationNode, argument: &str) -> anyhow::Result<Option<SlideEdge>> {
    let edge = match argument {
        "" => None,
        "left" => Some(SlideEdge::Left),
        "right" => Some(SlideEdge::Right),
        "top" => Some(SlideEdge::Top),
        "bottom" => Some(SlideEdge::Bottom),
        _ => bail!("animations.{} slide edge `{argument}` is invalid", node.name()),
    };
    Ok(edge)
}

#[cfg(test)]
fn resolve_for_test(
    patches: BTreeMap<AnimationNode, NodePatch>,
) -> anyhow::Result<AnimationsConfig> {
    resolve(patches, &builtin_beziers())
}

#[cfg(test)]
fn test_patch(
    enabled: Option<bool>,
    speed: Option<f64>,
    bezier: Option<&str>,
    style: Option<&str>,
) -> NodePatch {
    NodePatch {
        enabled,
        speed,
        bezier: bezier.map(str::to_string),
        style: style.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        compile, resolve_for_test, test_patch, AnimationNode, AnimationStyle, AnimationsConfigFile,
        SlideEdge,
    };

    #[test]
    fn example_defaults_use_hyprland_speed_and_inherited_move() {
        let config = compile(AnimationsConfigFile::default()).unwrap();
        assert_eq!(config.node(AnimationNode::Global).speed(), 10.0);
        assert_eq!(config.node(AnimationNode::Windows).duration_msec(), 479);
        assert_eq!(config.opening().duration_msec(), 410);
        assert_eq!(config.opening().style(), AnimationStyle::Popin { percent: 87 });
        assert_eq!(config.closing().duration_msec(), 149);
        assert_eq!(config.focus().duration_msec(), 303);
        assert_eq!(config.workspace().duration_msec(), 194);
        assert!(config.workspace().fades());
        assert_eq!(config.moving().speed(), config.node(AnimationNode::Windows).speed());
        assert_eq!(config.moving().easing(), config.node(AnimationNode::Windows).easing());
        assert_eq!(config.moving().style(), AnimationStyle::None);
        assert_eq!(config.node(AnimationNode::WorkspacesIn).duration_msec(), 121);
    }

    #[test]
    fn omitted_child_field_inherits_parent_and_explicit_value_overrides_it() {
        let mut patches = BTreeMap::new();
        patches.insert(
            AnimationNode::Global,
            test_patch(Some(false), Some(2.0), Some("linear"), None),
        );
        patches
            .insert(AnimationNode::WindowsIn, test_patch(Some(true), None, None, Some("gnomed")));
        let config = resolve_for_test(patches).unwrap();

        assert!(!config.node(AnimationNode::Windows).enabled());
        assert_eq!(config.node(AnimationNode::Windows).speed(), 2.0);
        assert_eq!(
            config.node(AnimationNode::Windows).easing(),
            config.node(AnimationNode::Global).easing()
        );
        assert!(config.opening().enabled());
        assert_eq!(config.opening().speed(), 2.0);
        assert_eq!(config.opening().style(), AnimationStyle::Gnomed);
    }

    #[test]
    fn user_field_replaces_only_that_example_field() {
        let config = compile(toml(
            r#"
            [windowsIn]
            style = "slide left"
            "#,
        ))
        .unwrap();
        assert_eq!(config.opening().duration_msec(), 410);
        assert_eq!(config.opening().style(), AnimationStyle::Slide { edge: Some(SlideEdge::Left) });
        assert_eq!(config.node(AnimationNode::Windows).duration_msec(), 479);
    }

    #[test]
    fn unknown_bezier_and_malformed_controls_fail() {
        let unknown = compile(toml(
            r#"
            [windows]
            bezier = "missing"
            "#,
        ));
        assert!(unknown.unwrap_err().to_string().contains("unknown bezier"));

        let points = compile(toml(
            r#"
            [bezier.overshoot]
            points = [1.2, 0.0, 0.2, 1.0]
            "#,
        ));
        assert!(points.unwrap_err().to_string().contains("within 0..=1"));

        let infinite = super::validate_bezier("broken", [0.0, f64::NAN, 1.0, 1.0]);
        assert!(infinite.unwrap_err().to_string().contains("non-finite"));
    }

    #[test]
    fn invalid_speed_style_and_unknown_node_fail() {
        let speed = compile(toml(
            r#"
            [fade]
            speed = 0
            "#,
        ));
        assert!(speed.unwrap_err().to_string().contains("speed"));

        let style = compile(toml(
            r#"
            [windowsOut]
            style = "popin 140%"
            "#,
        ));
        assert!(style.unwrap_err().to_string().contains("0..=100"));

        let node = toml::from_str::<AnimationsConfigFile>(
            r#"
            [layers]
            enabled = true
            "#,
        );
        assert!(node.is_err());
    }

    fn toml(input: &str) -> AnimationsConfigFile {
        toml::from_str(input).unwrap()
    }
}
