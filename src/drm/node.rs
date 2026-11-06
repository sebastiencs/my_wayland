//! Resolve the render node that belongs to one DRM device.
//!
//! The lookup starts from the device number of an existing DRM file descriptor
//! and reads only that device's sysfs directory. It never scans `/dev/dri` for
//! an arbitrary `renderD*` node.

use std::{
    fs::OpenOptions,
    io,
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
};

use thiserror::Error;

/// Kind of a kernel DRM node, identified by its exact device-node basename.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrmNodeKind {
    Primary,
    Render,
    Control,
}

/// Failure while resolving or opening the sibling render node.
///
/// `NoRenderNode`, `Open`, and `MismatchedDevice` are fallback conditions for
/// allocator setup. They are not by themselves fatal to the compositor.
#[derive(Debug, Error)]
pub(crate) enum RenderNodeError {
    #[error("DRM file descriptor is not a character device")]
    NotACharacterDevice,
    #[error("could not stat DRM file descriptor")]
    Stat(#[source] io::Error),
    #[error("could not read DRM node type from {}", path.display())]
    NodeKind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("DRM node name is not a primary, render, or control node")]
    UnrecognizedNode,
    #[error("could not list sibling DRM nodes in {}", path.display())]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("DRM device has no render node")]
    NoRenderNode,
    #[error("DRM device has no primary node")]
    NoPrimaryNode,
    #[error("could not open render node {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not reopen the current render node")]
    Reopen(#[source] io::Error),
    #[error("could not identify DRM device at {}", path.display())]
    DeviceIdentity {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("opened node does not belong to the source DRM device")]
    MismatchedDevice,
}

/// How to obtain an independent file description for the matching render node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RenderNodeAction<'a> {
    /// The source descriptor is already the render node. Reopen it instead of
    /// sharing its file description.
    ReopenCurrent,
    /// Open this basename below `/dev/dri`.
    OpenSibling(&'a str),
}

/// Classify `cardN`, `renderDN`, or `controlDN`. Any other spelling is rejected.
pub(super) fn classify_drm_node_name(name: &str) -> Option<DrmNodeKind> {
    if node_index(name, "card").is_some() {
        Some(DrmNodeKind::Primary)
    } else if node_index(name, "renderD").is_some() {
        Some(DrmNodeKind::Render)
    } else if node_index(name, "controlD").is_some() {
        Some(DrmNodeKind::Control)
    } else {
        None
    }
}

/// logind may take only an exact primary node. Render and control nodes stay
/// outside the session so the allocator can keep its own file description.
pub(crate) fn logind_takes_drm_node(sysname: &str) -> bool {
    classify_drm_node_name(sysname) == Some(DrmNodeKind::Primary)
}

/// Choose the render-node action from a source node and its sibling directory.
///
/// `sibling_names` must be the basenames from the source device's own `drm`
/// directory. Names outside that listing, including another GPU's render node,
/// are invisible to this choice.
pub(super) fn render_node_action<'a>(
    source_kind: DrmNodeKind,
    sibling_names: &'a [&'a str],
) -> Result<RenderNodeAction<'a>, RenderNodeError> {
    if source_kind == DrmNodeKind::Render {
        return Ok(RenderNodeAction::ReopenCurrent);
    }
    select_render_sibling(sibling_names)
        .map(RenderNodeAction::OpenSibling)
        .ok_or(RenderNodeError::NoRenderNode)
}

/// Return `/dev/dri/<name>` only when `name` is an exact render-node basename.
pub(super) fn render_node_path(name: &str) -> Option<PathBuf> {
    if classify_drm_node_name(name) != Some(DrmNodeKind::Render) {
        return None;
    }
    dri_node_path(name)
}

fn dri_node_path(name: &str) -> Option<PathBuf> {
    if classify_drm_node_name(name).is_none() {
        return None;
    }
    Some(Path::new("/dev/dri").join(name))
}

/// Compare two canonical DRM parent-device paths.
pub(super) fn same_drm_device(source_device: &Path, opened_device: &Path) -> bool {
    source_device == opened_device
}

/// Open a new file description for the primary node belonging to `fd`.
///
/// This never returns `fd` itself. A direct open of `/dev/dri/cardN` is tried
/// first. When the source is already primary and that open fails, the source
/// descriptor is reopened through `/proc/self/fd`, which works for a logind fd
/// whose device path is not directly openable.
pub(crate) fn open_independent_primary_node(fd: RawFd) -> Result<OwnedFd, RenderNodeError> {
    let source_dev = stat_rdev(fd)?;
    let source_kind = node_kind(source_dev)?;
    let opened = match source_kind {
        DrmNodeKind::Primary => open_primary_path_or_reopen(fd, source_dev)?,
        DrmNodeKind::Render | DrmNodeKind::Control => {
            let siblings = list_sibling_names(source_dev)?;
            let sibling_refs = siblings.iter().map(String::as_str).collect::<Vec<_>>();
            let name =
                select_primary_sibling(&sibling_refs).ok_or(RenderNodeError::NoPrimaryNode)?;
            let path = dri_node_path(name).expect("selected name is a DRM node");
            open_read_write(&path).map_err(|source| RenderNodeError::Open { path, source })?
        }
    };
    if !matches_primary(source_dev, &opened)? {
        return Err(RenderNodeError::MismatchedDevice);
    }
    Ok(opened)
}

/// Describe a DRM node as `<path> <major>:<minor>` for allocator logs.
pub(crate) fn describe_drm_node(fd: RawFd) -> String {
    let Ok(dev) = stat_rdev(fd) else {
        return format!("fd {fd} is not a DRM character device");
    };
    let major = nix::sys::stat::major(dev);
    let minor = nix::sys::stat::minor(dev);
    match devnode_path(dev) {
        Ok(path) => format!("{} {major}:{minor}", path.display()),
        Err(_) => format!("device {major}:{minor}"),
    }
}

/// Open an independent file description for the render node belonging to `fd`.
///
/// A primary or control node selects its sibling through sysfs. A render node
/// is reopened through `/proc/self/fd` so the result has a separate GEM-handle
/// namespace. This function does not fall back to the primary node.
pub(crate) fn open_matching_render_node(fd: RawFd) -> Result<OwnedFd, RenderNodeError> {
    let source_dev = stat_rdev(fd)?;
    let source_kind = node_kind(source_dev)?;
    let opened = match source_kind {
        DrmNodeKind::Render => reopen_current(fd)?,
        DrmNodeKind::Primary | DrmNodeKind::Control => {
            let siblings = list_sibling_names(source_dev)?;
            let sibling_refs = siblings.iter().map(String::as_str).collect::<Vec<_>>();
            let RenderNodeAction::OpenSibling(name) =
                render_node_action(source_kind, &sibling_refs)?
            else {
                unreachable!("primary and control nodes select a sibling");
            };
            open_sibling(name)?
        }
    };

    let opened_dev = stat_rdev(opened.as_raw_fd())?;
    if node_kind(opened_dev)? != DrmNodeKind::Render {
        return Err(RenderNodeError::MismatchedDevice);
    }
    if !same_drm_device(&device_identity(source_dev)?, &device_identity(opened_dev)?) {
        return Err(RenderNodeError::MismatchedDevice);
    }
    Ok(opened)
}

fn select_primary_sibling<'a>(names: &'a [&'a str]) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| classify_drm_node_name(name) == Some(DrmNodeKind::Primary))
        .min_by(|left, right| {
            node_index(left, "card")
                .cmp(&node_index(right, "card"))
                .then(left.cmp(right))
        })
}

fn select_render_sibling<'a>(names: &'a [&'a str]) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| classify_drm_node_name(name) == Some(DrmNodeKind::Render))
        .min_by(|left, right| {
            node_index(left, "renderD")
                .cmp(&node_index(right, "renderD"))
                .then(left.cmp(right))
        })
}

fn node_index(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn stat_rdev(fd: RawFd) -> Result<libc::dev_t, RenderNodeError> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == -1 {
        return Err(RenderNodeError::Stat(io::Error::last_os_error()));
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
        return Err(RenderNodeError::NotACharacterDevice);
    }
    Ok(stat.st_rdev)
}

fn node_kind(dev: libc::dev_t) -> Result<DrmNodeKind, RenderNodeError> {
    let devname = read_devname(dev)?;
    let basename = devname.rsplit('/').next().unwrap_or(devname.as_str());
    classify_drm_node_name(basename).ok_or(RenderNodeError::UnrecognizedNode)
}

fn devnode_path(dev: libc::dev_t) -> Result<PathBuf, RenderNodeError> {
    let devname = read_devname(dev)?;
    if devname.starts_with('/') {
        Ok(PathBuf::from(devname))
    } else {
        Ok(Path::new("/dev").join(devname))
    }
}

fn read_devname(dev: libc::dev_t) -> Result<String, RenderNodeError> {
    let path = sysfs_path(dev, "uevent");
    let text = std::fs::read_to_string(&path)
        .map_err(|source| RenderNodeError::NodeKind { path: path.clone(), source })?;
    text.lines()
        .find_map(|line| line.strip_prefix("DEVNAME="))
        .map(str::to_string)
        .ok_or(RenderNodeError::UnrecognizedNode)
}

fn open_primary_path_or_reopen(
    fd: RawFd,
    source_dev: libc::dev_t,
) -> Result<OwnedFd, RenderNodeError> {
    if let Ok(path) = devnode_path(source_dev) {
        if let Ok(opened) = open_read_write(&path) {
            if matches_primary(source_dev, &opened).unwrap_or(false) {
                return Ok(opened);
            }
            log::debug!(
                "opened primary path {} does not match the source DRM device",
                path.display()
            );
        }
    }
    reopen_current(fd)
}

fn matches_primary(source_dev: libc::dev_t, opened: &OwnedFd) -> Result<bool, RenderNodeError> {
    let opened_dev = stat_rdev(opened.as_raw_fd())?;
    Ok(node_kind(opened_dev)? == DrmNodeKind::Primary
        && same_drm_device(&device_identity(source_dev)?, &device_identity(opened_dev)?))
}

fn list_sibling_names(dev: libc::dev_t) -> Result<Vec<String>, RenderNodeError> {
    let path = sysfs_path(dev, "device/drm");
    let entries = std::fs::read_dir(&path)
        .map_err(|source| RenderNodeError::ReadDirectory { path: path.clone(), source })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|source| RenderNodeError::ReadDirectory { path: path.clone(), source })?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn open_sibling(name: &str) -> Result<OwnedFd, RenderNodeError> {
    let path = render_node_path(name).expect("selected name is a render node");
    open_read_write(&path).map_err(|source| RenderNodeError::Open { path, source })
}

fn reopen_current(fd: RawFd) -> Result<OwnedFd, RenderNodeError> {
    let path = format!("/proc/self/fd/{fd}");
    open_read_write(Path::new(&path)).map_err(RenderNodeError::Reopen)
}

fn open_read_write(path: &Path) -> io::Result<OwnedFd> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map(OwnedFd::from)
}

fn device_identity(dev: libc::dev_t) -> Result<PathBuf, RenderNodeError> {
    let path = sysfs_path(dev, "device");
    std::fs::canonicalize(&path).map_err(|source| RenderNodeError::DeviceIdentity { path, source })
}

fn sysfs_path(dev: libc::dev_t, suffix: &str) -> PathBuf {
    let major = nix::sys::stat::major(dev);
    let minor = nix::sys::stat::minor(dev);
    PathBuf::from(format!("/sys/dev/char/{major}:{minor}/{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::{
        classify_drm_node_name, logind_takes_drm_node, open_matching_render_node,
        render_node_action, render_node_path, same_drm_device, DrmNodeKind, RenderNodeAction,
        RenderNodeError,
    };

    #[test]
    fn classifies_exact_kernel_node_names() {
        assert_eq!(classify_drm_node_name("card0"), Some(DrmNodeKind::Primary));
        assert_eq!(classify_drm_node_name("card10"), Some(DrmNodeKind::Primary));
        assert_eq!(classify_drm_node_name("renderD128"), Some(DrmNodeKind::Render));
        assert_eq!(classify_drm_node_name("renderD0"), Some(DrmNodeKind::Render));
        assert_eq!(classify_drm_node_name("controlD64"), Some(DrmNodeKind::Control));
    }

    #[test]
    fn rejects_partial_or_decorated_node_names() {
        for name in [
            "card",
            "card0a",
            "renderD",
            "renderD128x",
            "control",
            "controlD",
            "render",
            "/dev/dri/renderD128",
            "../renderD128",
            "renderD128/extra",
            "",
        ] {
            assert_eq!(classify_drm_node_name(name), None, "{name}");
        }
    }

    #[test]
    fn selects_lowest_sibling_render_node_numerically() {
        let names = ["renderD130", "card1", "controlD65", "renderD128", "renderD2"];
        assert_eq!(
            render_node_action(DrmNodeKind::Primary, &names).unwrap(),
            RenderNodeAction::OpenSibling("renderD2")
        );
        assert_eq!(
            render_node_action(DrmNodeKind::Control, &names).unwrap(),
            RenderNodeAction::OpenSibling("renderD2")
        );
    }

    #[test]
    fn selects_lowest_primary_sibling_numerically() {
        let names = ["card10", "renderD128", "card2", "controlD64"];
        assert_eq!(super::select_primary_sibling(&names), Some("card2"));
    }

    #[test]
    fn numeric_order_beats_lexical_order() {
        let names = ["renderD10", "renderD2"];
        assert_eq!(
            render_node_action(DrmNodeKind::Primary, &names).unwrap(),
            RenderNodeAction::OpenSibling("renderD2")
        );
    }

    #[test]
    fn ignores_render_nodes_absent_from_the_sibling_directory() {
        let names = ["card1", "controlD65", "renderD130"];
        assert_eq!(
            render_node_action(DrmNodeKind::Primary, &names).unwrap(),
            RenderNodeAction::OpenSibling("renderD130")
        );
    }

    #[test]
    fn missing_sibling_is_a_fallback_condition() {
        let names = ["card1", "controlD65", ".", ".."];
        assert!(matches!(
            render_node_action(DrmNodeKind::Primary, &names),
            Err(RenderNodeError::NoRenderNode)
        ));
    }

    #[test]
    fn an_existing_render_node_is_reopened_instead_of_sharing_its_descriptor() {
        let names = ["card1", "renderD999"];
        assert_eq!(
            render_node_action(DrmNodeKind::Render, &names).unwrap(),
            RenderNodeAction::ReopenCurrent
        );
    }

    #[test]
    fn render_path_rejects_anything_except_a_render_basename() {
        assert_eq!(
            render_node_path("renderD128"),
            Some(std::path::PathBuf::from("/dev/dri/renderD128"))
        );
        assert_eq!(render_node_path("../renderD128"), None);
        assert_eq!(render_node_path("card1"), None);
    }

    #[test]
    fn same_device_requires_the_same_canonical_parent() {
        let device = std::path::Path::new("/sys/devices/pci0000:00/0000:0c:00.0");
        let other = std::path::Path::new("/sys/devices/pci0000:00/0000:0d:00.0");
        assert!(same_drm_device(device, device));
        assert!(!same_drm_device(device, other));
    }

    #[test]
    fn logind_takes_only_exact_primary_nodes() {
        assert!(logind_takes_drm_node("card0"));
        assert!(logind_takes_drm_node("card1"));
        assert!(!logind_takes_drm_node("renderD128"));
        assert!(!logind_takes_drm_node("controlD64"));
        assert!(!logind_takes_drm_node("card"));
        assert!(!logind_takes_drm_node("card1extra"));
        assert!(!logind_takes_drm_node("/dev/dri/card1"));
    }

    #[test]
    fn invalid_file_descriptor_cannot_resolve_a_render_node() {
        assert!(matches!(open_matching_render_node(-1), Err(RenderNodeError::Stat(_))));
    }
}
