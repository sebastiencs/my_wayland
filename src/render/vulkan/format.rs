use std::collections::HashMap;

use {ash::vk, drm::buffer::DrmFourcc};

use crate::{
    drm::{ARGB8888, XRGB8888},
    render::{FormatSupport, RendererCapabilities},
};

use super::instance::VulkanInstance;

const INVALID_MODIFIER: u64 = 0x00ff_ffff_ffff_ffff;

#[derive(Clone, Copy, Debug)]
pub(super) struct MaxExtent {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ModifierSupport {
    pub planes: u32,
    pub disjoint: bool,
    pub sample_max: Option<MaxExtent>,
    pub render_max: Option<MaxExtent>,
}

#[derive(Clone, Debug)]
pub(super) struct FormatSet {
    pub by_fourcc: HashMap<DrmFourcc, HashMap<u64, ModifierSupport>>,
    pub capabilities: RendererCapabilities,
}

pub(super) fn query_formats(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
) -> Result<FormatSet, String> {
    let mut by_fourcc = HashMap::new();
    for fourcc in [ARGB8888.drm, XRGB8888.drm] {
        let modifiers = query_fourcc(instance, physical_device, fourcc)?;
        if !modifiers.is_empty() {
            by_fourcc.insert(fourcc, modifiers);
        }
    }
    let capabilities = capabilities_from(&by_fourcc);
    Ok(FormatSet { by_fourcc, capabilities })
}

fn query_fourcc(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
    fourcc: DrmFourcc,
) -> Result<HashMap<u64, ModifierSupport>, String> {
    let vk_format = vk_format(fourcc);
    let count = modifier_count(instance, physical_device, vk_format);
    if count == 0 {
        return Ok(HashMap::new());
    }
    let listed = modifier_properties(instance, physical_device, vk_format, count)?;
    let mut modifiers = HashMap::new();
    for modifier in listed {
        if modifier.drm_format_modifier == INVALID_MODIFIER
            || modifier.drm_format_modifier_plane_count == 0
        {
            continue;
        }
        let sample_max = image_extent(
            instance,
            physical_device,
            vk_format,
            &modifier,
            vk::FormatFeatureFlags::SAMPLED_IMAGE,
            vk::ImageUsageFlags::SAMPLED,
        )?;
        // Jay advertises a modifier as a framebuffer only when it can also be copied.
        let render_max = image_extent(
            instance,
            physical_device,
            vk_format,
            &modifier,
            vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::TRANSFER_SRC,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        if sample_max.is_none() && render_max.is_none() {
            continue;
        }
        modifiers.insert(
            modifier.drm_format_modifier,
            ModifierSupport {
                planes: modifier.drm_format_modifier_plane_count,
                disjoint: modifier
                    .drm_format_modifier_tiling_features
                    .contains(vk::FormatFeatureFlags::DISJOINT),
                sample_max,
                render_max,
            },
        );
    }
    Ok(modifiers)
}

fn modifier_count(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
) -> u32 {
    let mut modifiers = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut modifiers);
    unsafe {
        instance.instance.get_physical_device_format_properties2(
            physical_device,
            format,
            &mut properties,
        );
    }
    modifiers.drm_format_modifier_count
}

fn modifier_properties(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
    count: u32,
) -> Result<Vec<vk::DrmFormatModifierPropertiesEXT>, String> {
    let mut listed = vec![vk::DrmFormatModifierPropertiesEXT::default(); count as usize];
    let reported = {
        let mut modifiers = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut listed);
        let mut properties = vk::FormatProperties2::default().push_next(&mut modifiers);
        unsafe {
            instance.instance.get_physical_device_format_properties2(
                physical_device,
                format,
                &mut properties,
            );
        }
        modifiers.drm_format_modifier_count
    };
    if reported as usize > listed.len() {
        return Err(format!("vulkan reported {count} DRM modifiers and then {reported}"));
    }
    listed.truncate(reported as usize);
    Ok(listed)
}

fn image_extent(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
    modifier: &vk::DrmFormatModifierPropertiesEXT,
    features: vk::FormatFeatureFlags,
    usage: vk::ImageUsageFlags,
) -> Result<Option<MaxExtent>, String> {
    if !modifier
        .drm_format_modifier_tiling_features
        .contains(features)
    {
        return Ok(None);
    }
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier.drm_format_modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
        .ty(vk::ImageType::TYPE_2D)
        .format(format)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .push_next(&mut external_info)
        .push_next(&mut modifier_info);
    let mut external_properties = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external_properties);
    let result = unsafe {
        instance
            .instance
            .get_physical_device_image_format_properties2(
                physical_device,
                &format_info,
                &mut properties,
            )
    };
    if let Err(error) = result {
        return match error {
            vk::Result::ERROR_FORMAT_NOT_SUPPORTED => Ok(None),
            other => Err(format!("failed to query vulkan image format properties: {other}")),
        };
    }
    let extent = properties.image_format_properties.max_extent;
    let importable = external_properties
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE);
    if !importable {
        return Ok(None);
    }
    if extent.width == 0 || extent.height == 0 {
        return Ok(None);
    }
    Ok(Some(MaxExtent { width: extent.width, height: extent.height }))
}

fn capabilities_from(
    formats: &HashMap<DrmFourcc, HashMap<u64, ModifierSupport>>,
) -> RendererCapabilities {
    let mut fourccs: Vec<DrmFourcc> = formats.keys().copied().collect();
    fourccs.sort_by_key(|fourcc| format!("{fourcc:?}"));
    let sample_formats = listed(formats, &fourccs, |modifier| modifier.sample_max.is_some());
    let render_formats = listed(formats, &fourccs, |modifier| modifier.render_max.is_some());
    RendererCapabilities { sample_formats, render_formats, explicit_sync: true }
}

fn listed(
    formats: &HashMap<DrmFourcc, HashMap<u64, ModifierSupport>>,
    fourccs: &[DrmFourcc],
    accept: impl Fn(&ModifierSupport) -> bool,
) -> Vec<FormatSupport> {
    fourccs
        .iter()
        .filter_map(|fourcc| {
            let mut modifiers: Vec<u64> = formats[fourcc]
                .iter()
                .filter(|(_, modifier)| accept(modifier))
                .map(|(modifier, _)| *modifier)
                .collect();
            if modifiers.is_empty() {
                return None;
            }
            modifiers.sort_unstable();
            Some(FormatSupport { fourcc: *fourcc, modifiers })
        })
        .collect()
}

pub(super) fn vk_format(fourcc: DrmFourcc) -> vk::Format {
    match fourcc {
        DrmFourcc::Argb8888 | DrmFourcc::Xrgb8888 => vk::Format::B8G8R8A8_UNORM,
        _ => vk::Format::UNDEFINED,
    }
}
