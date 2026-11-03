use std::{fs::OpenOptions, os::fd::OwnedFd, path::PathBuf, rc::Rc};

use drm::buffer::DrmFourcc;

use crate::{
    config::{RendererSelection, RoundingPower},
    drm::DrmFd,
    gbm::{GbmDevice, UseFlags},
    region::{DamageRegion, RectI},
    render::{
        create_renderer, gles::GlesRenderer, EffectElement, FrameElement, FramePhase, GraphicsApi,
        OutputFrameRecipe, RenderFrame, RenderSyncMode, Renderer, ShmImage, Texture,
        TextureElement, TextureMask, TextureSource,
    },
    state::RootSurfaceClip,
    transform::{Mat3, OutputPresentationTransform},
};

struct Device {
    renderer: GlesRenderer,
    gbm: GbmDevice,
}

fn open_render_node() -> Option<OwnedFd> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return None;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("renderD") || name.starts_with("card"))
        })
        .collect();
    paths.sort_by_key(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        (usize::from(!name.starts_with("renderD")), name.to_string())
    });
    paths.into_iter().find_map(|path| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .ok()
            .map(OwnedFd::from)
    })
}

fn device() -> Option<Device> {
    crate::render::gles::init().ok()?;
    let fd = open_render_node()?;
    let drm = DrmFd::from_owned(fd);
    let gbm = GbmDevice::new(&drm).ok()?;
    let renderer = GlesRenderer::from_gbm(&gbm).ok()?;
    Some(Device { renderer, gbm })
}

fn writable_export(device: &Device, format: DrmFourcc) -> Option<crate::gbm::BufferObject> {
    let attempts = [
        UseFlags::WRITE | UseFlags::LINEAR | UseFlags::RENDERING,
        UseFlags::WRITE | UseFlags::LINEAR,
        UseFlags::CURSOR | UseFlags::WRITE | UseFlags::LINEAR,
    ];
    attempts
        .into_iter()
        .find_map(|flags| device.gbm.try_create_bo_with_size(2, 2, format, flags))
}

fn solid(color: [u8; 4], width: i32, height: i32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((width * height) as usize * 4);
    for _ in 0..width * height {
        bytes.extend_from_slice(&color);
    }
    bytes
}

fn texture_element(texture: Rc<dyn Texture>, width: i32, height: i32) -> FrameElement {
    let rect = RectI::new(0, 0, width, height).expect("rect");
    FrameElement::Texture {
        phase: FramePhase::Content,
        element: TextureElement {
            source: TextureSource::Texture,
            texture,
            x: 0,
            y: 0,
            width,
            height,
            tex_transform: Mat3::identity(),
            damage: DamageRegion::from_rect(rect),
            blend: false,
            alpha: u8::MAX,
            mask: TextureMask::None,
        },
    }
}

fn render_solid(
    renderer: &mut GlesRenderer,
    target: &mut dyn crate::render::RenderTarget,
    element: FrameElement,
    damage: RectI,
    preserve_contents: bool,
    sync_mode: RenderSyncMode,
    blur_reference: Option<(i32, i32)>,
) -> crate::render::RenderCompletion {
    let recipe = OutputFrameRecipe { elements: vec![element] };
    let damage_region = DamageRegion::from_rect(damage);
    renderer
        .render(
            target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage_region,
                preserve_contents,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode,
                blur_reference,
            },
        )
        .expect("render")
}

fn pixel(bytes: &[u8], stride: usize, x: i32, y: i32) -> [u8; 4] {
    let start = y as usize * stride + x as usize * 4;
    bytes[start..start + 4].try_into().expect("pixel")
}

#[test]
fn shm_upload_renders_its_color() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let bytes = solid([0, 255, 0, 255], 2, 2);
    let texture = device
        .renderer
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm import");
    let mut target = device
        .renderer
        .create_offscreen_target(2, 2)
        .expect("offscreen");
    render_solid(
        &mut device.renderer,
        &mut *target,
        texture_element(texture, 2, 2),
        RectI::new(0, 0, 2, 2).unwrap(),
        false,
        RenderSyncMode::BlockingFinishFallback,
        None,
    );
    let mut read = vec![0u8; 16];
    device
        .renderer
        .readback(&*target, RectI::new(0, 0, 2, 2).unwrap(), 8, &mut read)
        .expect("readback");
    assert_eq!(pixel(&read, 8, 0, 0), [0, 255, 0, 255]);
}

#[test]
fn dmabuf_import_is_cached_and_renders() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let cached = device.gbm.create_bo_with_size(
        2,
        2,
        DrmFourcc::Argb8888,
        UseFlags::RENDERING | UseFlags::SCANOUT,
        &[],
    );
    let cached_dmabuf = cached.dmabuf().expect("exported dmabuf");
    let first = device
        .renderer
        .import_dmabuf(cached_dmabuf)
        .expect("dmabuf import");
    let second = device
        .renderer
        .import_dmabuf(cached_dmabuf)
        .expect("cached import");
    assert!(Rc::ptr_eq(&first, &second));
    let Some(bo) = writable_export(&device, DrmFourcc::Argb8888) else {
        eprintln!("skipping dma-buf pixel import: no CPU-writable exportable buffer");
        return;
    };
    let dmabuf = bo.dmabuf().expect("exported dmabuf");
    let stride = dmabuf.planes[0].stride as usize;
    let mut bytes = vec![0u8; stride * 2];
    for y in 0..2 {
        for x in 0..2 {
            bytes[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[0, 0, 255, 255]);
        }
    }
    bo.write(&bytes);
    let first = device
        .renderer
        .import_dmabuf(dmabuf)
        .expect("dmabuf import");
    let second = device
        .renderer
        .import_dmabuf(dmabuf)
        .expect("cached import");
    assert!(Rc::ptr_eq(&first, &second));
    let mut target = device
        .renderer
        .create_offscreen_target(2, 2)
        .expect("offscreen");
    render_solid(
        &mut device.renderer,
        &mut *target,
        texture_element(first, 2, 2),
        RectI::new(0, 0, 2, 2).unwrap(),
        false,
        RenderSyncMode::BlockingFinishFallback,
        None,
    );
    let mut read = vec![0u8; 16];
    device
        .renderer
        .readback(&*target, RectI::new(0, 0, 2, 2).unwrap(), 8, &mut read)
        .expect("readback");
    assert_eq!(pixel(&read, 8, 1, 1), [0, 0, 255, 255]);
}

#[test]
fn effects_darken_and_damage_keeps_untouched_pixels() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let green = solid([0, 255, 0, 255], 4, 2);
    let texture = device
        .renderer
        .import_shm(ShmImage {
            bytes: &green,
            width: 4,
            height: 2,
            stride: 16,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm import");
    let rect = RectI::new(0, 0, 4, 2).unwrap();
    let clip = RootSurfaceClip::new(rect, 0, RoundingPower::DEFAULT);
    let recipe = OutputFrameRecipe {
        elements: vec![
            texture_element(texture, 4, 2),
            FrameElement::Effect {
                phase: FramePhase::Overlay,
                element: EffectElement::DimOverlay { clip, alpha: 128 },
            },
        ],
    };
    let mut target = device
        .renderer
        .create_offscreen_target(4, 2)
        .expect("offscreen");
    let damage = DamageRegion::from_rect(rect);
    device
        .renderer
        .render(
            &mut *target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage,
                preserve_contents: false,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: Some((8, 8)),
            },
        )
        .expect("effect render");
    let mut read = vec![0u8; 32];
    device
        .renderer
        .readback(&*target, rect, 16, &mut read)
        .expect("readback");
    assert!(pixel(&read, 16, 1, 0)[1] < 250, "dim overlay should darken green");

    let bo = device.gbm.create_bo_with_size(
        4,
        2,
        DrmFourcc::Xrgb8888,
        UseFlags::RENDERING | UseFlags::SCANOUT,
        &[],
    );
    let output = bo.dmabuf().expect("output dmabuf");
    let mut output_target = device
        .renderer
        .import_output_target(output)
        .expect("output target");
    let repaint = solid([0, 255, 0, 255], 4, 2);
    let painted = device
        .renderer
        .import_shm(ShmImage {
            bytes: &repaint,
            width: 4,
            height: 2,
            stride: 16,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("repaint import");
    render_solid(
        &mut device.renderer,
        &mut *output_target,
        texture_element(painted, 4, 2),
        rect,
        false,
        RenderSyncMode::BlockingFinishFallback,
        None,
    );
    let left = RectI::new(0, 0, 2, 2).unwrap();
    let empty = OutputFrameRecipe { elements: Vec::new() };
    let left_damage = DamageRegion::from_rect(left);
    device
        .renderer
        .render(
            &mut *output_target,
            RenderFrame {
                recipe: &empty,
                damage: &left_damage,
                preserve_contents: true,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: None,
            },
        )
        .expect("damage clear");
    let mut preserved = vec![0u8; 32];
    device
        .renderer
        .readback(&*output_target, rect, 16, &mut preserved)
        .expect("damage readback");
    assert_eq!(pixel(&preserved, 16, 0, 0)[2], 255, "damaged pixels clear to red");
    assert_eq!(pixel(&preserved, 16, 3, 0), [0, 255, 0, 255]);
}

#[test]
fn explicit_sync_render_exports_a_fence_when_supported() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let bo = device.gbm.create_bo_with_size(
        2,
        2,
        DrmFourcc::Xrgb8888,
        UseFlags::RENDERING | UseFlags::SCANOUT,
        &[],
    );
    let dmabuf = bo.dmabuf().expect("output dmabuf");
    let mut target = device
        .renderer
        .import_output_target(dmabuf)
        .expect("output target");
    let bytes = solid([255, 0, 0, 255], 2, 2);
    let texture = device
        .renderer
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm import");
    let sync_mode = if device.renderer.capabilities().explicit_sync {
        RenderSyncMode::ExplicitInFence
    } else {
        RenderSyncMode::BlockingFinishFallback
    };
    let completion = render_solid(
        &mut device.renderer,
        &mut *target,
        texture_element(texture, 2, 2),
        RectI::new(0, 0, 2, 2).unwrap(),
        false,
        sync_mode,
        None,
    );
    assert_eq!(completion.in_fence_fd.is_some(), !completion.used_blocking_fallback);
    if device.renderer.capabilities().explicit_sync {
        assert!(completion.in_fence_fd.is_some(), "explicit sync should export a render fence");
    }
}

#[test]
fn capture_reads_back_the_window_color() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let bytes = solid([0, 255, 0, 255], 2, 2);
    let texture = device
        .renderer
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm import");
    let captured = device
        .renderer
        .capture_transparent(
            &[texture_element(texture, 2, 2)],
            2,
            2,
            (8, 8),
            OutputPresentationTransform::FlipY,
        )
        .expect("capture");
    let mut target = device
        .renderer
        .create_offscreen_target(2, 2)
        .expect("offscreen");
    render_solid(
        &mut device.renderer,
        &mut *target,
        texture_element(captured, 2, 2),
        RectI::new(0, 0, 2, 2).unwrap(),
        false,
        RenderSyncMode::BlockingFinishFallback,
        Some((8, 8)),
    );
    let mut read = vec![0u8; 16];
    device
        .renderer
        .readback(&*target, RectI::new(0, 0, 2, 2).unwrap(), 8, &mut read)
        .expect("readback");
    assert_eq!(pixel(&read, 8, 0, 1), [0, 255, 0, 255]);
}

#[test]
fn fullscreen_client_buffer_can_be_composited() {
    let Some(mut device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let Some(bo) = writable_export(&device, DrmFourcc::Xrgb8888) else {
        eprintln!("skipping dma-buf composite: no CPU-writable exportable buffer");
        return;
    };
    let dmabuf = bo.dmabuf().expect("client dmabuf");
    let stride = dmabuf.planes[0].stride as usize;
    let mut bytes = vec![0u8; stride * 2];
    for y in 0..2 {
        for x in 0..2 {
            bytes[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[255, 128, 0, 255]);
        }
    }
    bo.write(&bytes);
    let texture = device.renderer.import_dmabuf(dmabuf).expect("import");
    let mut target = device
        .renderer
        .create_offscreen_target(2, 2)
        .expect("offscreen");
    render_solid(
        &mut device.renderer,
        &mut *target,
        texture_element(texture, 2, 2),
        RectI::new(0, 0, 2, 2).unwrap(),
        false,
        RenderSyncMode::BlockingFinishFallback,
        None,
    );
    let mut read = vec![0u8; 16];
    device
        .renderer
        .readback(&*target, RectI::new(0, 0, 2, 2).unwrap(), 8, &mut read)
        .expect("readback");
    assert_eq!(pixel(&read, 8, 0, 0), [255, 128, 0, 255]);
}

#[test]
fn factory_uses_opengl_and_rejects_vulkan() {
    let Some(device) = device() else {
        eprintln!("skipping GLES smoke: no DRM render node");
        return;
    };
    let opengl = create_renderer(RendererSelection::OpenGl, &device.gbm, &device.gbm)
        .expect("opengl renderer");
    assert_eq!(opengl.api(), GraphicsApi::OpenGl);
    let auto =
        create_renderer(RendererSelection::Auto, &device.gbm, &device.gbm).expect("auto renderer");
    assert_eq!(auto.api(), GraphicsApi::OpenGl);
    let error = create_renderer(RendererSelection::Vulkan, &device.gbm, &device.gbm)
        .expect_err("vulkan is not implemented");
    assert!(error.contains("not implemented"));
}
