use std::{fs::OpenOptions, os::fd::OwnedFd, path::PathBuf, rc::Rc};

use drm::buffer::DrmFourcc;

use crate::{
    config::{RendererSelection, RgbaColor, RoundingPower, ShadowPower},
    drm::DrmFd,
    gbm::{GbmDevice, UseFlags},
    region::{DamageRegion, RectI},
    render::{
        create_renderer, BlurVisualStyle, BorderGradient, EffectElement, FrameElement, FramePhase,
        OutputFrameRecipe, RenderFrame, RenderSyncMode, Renderer, RoundedRing, ShadowVisualStyle,
        ShmImage, Texture, TextureElement, TextureMask, TextureSource,
    },
    state::RootSurfaceClip,
    transform::{Mat3, OutputPresentationTransform},
};

struct Pair {
    gl: Box<dyn Renderer>,
    vulkan: Box<dyn Renderer>,
    gbm: GbmDevice,
}

fn open_render_node() -> Option<OwnedFd> {
    let entries = std::fs::read_dir("/dev/dri").ok()?;
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

fn pair() -> Option<Pair> {
    crate::render::gles::init().ok()?;
    let fd = open_render_node()?;
    let drm = DrmFd::from_owned(fd);
    let gbm = GbmDevice::new(&drm).ok()?;
    let gl = create_renderer(RendererSelection::OpenGl, &gbm, &gbm).ok()?;
    let vulkan = create_renderer(RendererSelection::Vulkan, &gbm, &gbm).ok()?;
    Some(Pair { gl, vulkan, gbm })
}

fn solid(color: [u8; 4], width: i32, height: i32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((width * height) as usize * 4);
    for _ in 0..width * height {
        bytes.extend_from_slice(&color);
    }
    bytes
}

fn element(
    texture: Rc<dyn Texture>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    alpha: u8,
    mask: TextureMask,
) -> FrameElement {
    let rect = RectI::new(x, y, width, height).expect("rect");
    FrameElement::Texture {
        phase: FramePhase::Content,
        element: TextureElement {
            source: TextureSource::Texture,
            texture,
            x,
            y,
            width,
            height,
            tex_transform: Mat3::identity(),
            damage: DamageRegion::from_rect(rect),
            blend: alpha < u8::MAX || !matches!(mask, TextureMask::None),
            alpha,
            mask,
        },
    }
}

fn paint(renderer: &mut dyn Renderer, element: FrameElement, width: i32, height: i32) -> Vec<u8> {
    let recipe = OutputFrameRecipe { elements: vec![element] };
    let damage = DamageRegion::from_rect(RectI::new(0, 0, width, height).expect("damage"));
    let mut target = renderer
        .create_offscreen_target(width, height)
        .expect("offscreen");
    renderer
        .render(
            &mut *target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage,
                preserve_contents: false,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: None,
            },
        )
        .expect("render");
    let stride = width as usize * 4;
    let mut bytes = vec![0_u8; stride * height as usize];
    renderer
        .readback(&*target, RectI::new(0, 0, width, height).expect("read"), stride, &mut bytes)
        .expect("readback");
    bytes
}

fn assert_close(left: &[u8], right: &[u8], tolerance: u8) {
    assert_eq!(left.len(), right.len());
    for (index, (gles, vulkan)) in left.iter().zip(right).enumerate() {
        let delta = gles.abs_diff(*vulkan);
        assert!(
            delta <= tolerance,
            "pixel byte {index}: gles {gles} vulkan {vulkan} differ by {delta}"
        );
    }
}

#[test]
fn vulkan_matches_gles_textured_quads() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan pixel comparison: no matching DRM node");
        return;
    };
    let _ = pair.gbm;

    let green = [0, 255, 0, 255];
    let bytes = solid(green, 2, 2);
    let gl_texture = pair
        .gl
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("gles shm");
    let vk_texture = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("vulkan shm");
    assert_close(
        &paint(&mut *pair.gl, element(gl_texture, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2),
        &paint(
            &mut *pair.vulkan,
            element(vk_texture, 0, 0, 2, 2, u8::MAX, TextureMask::None),
            2,
            2,
        ),
        0,
    );

    let mut rows = solid(green, 2, 1);
    rows.extend(solid([255, 0, 0, 255], 2, 1));
    let gl_rows = pair
        .gl
        .import_shm(ShmImage {
            bytes: &rows,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("gles rows");
    let vk_rows = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &rows,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("vulkan rows");
    let gles_rows =
        paint(&mut *pair.gl, element(gl_rows, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2);
    let vulkan_rows =
        paint(&mut *pair.vulkan, element(vk_rows, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2);
    assert_ne!(gles_rows[..4], gles_rows[8..12], "the two rows must differ");
    assert_close(&gles_rows, &vulkan_rows, 0);

    let gl_dot = pair
        .gl
        .import_shm(ShmImage {
            bytes: &green,
            width: 1,
            height: 1,
            stride: 4,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("gles dot");
    let vk_dot = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &green,
            width: 1,
            height: 1,
            stride: 4,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("vulkan dot");
    let gles_dot =
        paint(&mut *pair.gl, element(gl_dot, 0, 0, 1, 1, u8::MAX, TextureMask::None), 2, 2);
    let vulkan_dot =
        paint(&mut *pair.vulkan, element(vk_dot, 0, 0, 1, 1, u8::MAX, TextureMask::None), 2, 2);
    assert_eq!(gles_dot[..4], green);
    assert_eq!(&gles_dot[4..8], &[0, 0, 0, 0]);
    assert_close(&gles_dot, &vulkan_dot, 0);

    let rgba = solid([255, 0, 0, 255], 2, 2);
    let gl_rgba = pair.gl.upload_rgba(&rgba, 2, 2).expect("gles rgba");
    let vk_rgba = pair.vulkan.upload_rgba(&rgba, 2, 2).expect("vulkan rgba");
    let rgba_bytes =
        paint(&mut *pair.gl, element(gl_rgba, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2);
    assert_eq!(&rgba_bytes[..4], &[0, 0, 255, 255]);
    assert_close(
        &rgba_bytes,
        &paint(&mut *pair.vulkan, element(vk_rgba, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2),
        0,
    );

    let gl_alpha = pair
        .gl
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("gles alpha");
    let vk_alpha = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("vulkan alpha");
    assert_close(
        &paint(&mut *pair.gl, element(gl_alpha, 0, 0, 2, 2, 128, TextureMask::None), 2, 2),
        &paint(&mut *pair.vulkan, element(vk_alpha, 0, 0, 2, 2, 128, TextureMask::None), 2, 2),
        1,
    );

    let clip =
        RootSurfaceClip::new(RectI::new(0, 0, 4, 4).expect("clip"), 8, RoundingPower::default());
    let mask = TextureMask::RoundedClip { clip };
    let rounded = solid(green, 4, 4);
    let gl_round = pair
        .gl
        .import_shm(ShmImage {
            bytes: &rounded,
            width: 4,
            height: 4,
            stride: 16,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("gles clip");
    let vk_round = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &rounded,
            width: 4,
            height: 4,
            stride: 16,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("vulkan clip");
    let gles_round =
        paint(&mut *pair.gl, element(gl_round, 0, 0, 4, 4, u8::MAX, mask.clone()), 4, 4);
    let vulkan_round = paint(&mut *pair.vulkan, element(vk_round, 0, 0, 4, 4, u8::MAX, mask), 4, 4);
    assert_eq!(&gles_round[..4], &[0, 0, 0, 0]);
    assert_eq!(&gles_round[8 * 4 + 8..8 * 4 + 12], &green);
    assert_close(&gles_round, &vulkan_round, 1);
}

#[test]
fn vulkan_draws_a_texture_when_the_frame_also_has_an_effect() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan effect skip: no matching DRM node");
        return;
    };
    let _ = pair.gbm;
    let green = [0, 255, 0, 255];
    let bytes = solid(green, 2, 2);
    let texture = pair
        .vulkan
        .import_shm(ShmImage {
            bytes: &bytes,
            width: 2,
            height: 2,
            stride: 8,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm");
    let rect = RectI::new(0, 0, 2, 2).expect("rect");
    let recipe = OutputFrameRecipe {
        elements: vec![
            element(texture, 0, 0, 2, 2, u8::MAX, TextureMask::None),
            FrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::DimOverlay {
                    clip: RootSurfaceClip::new(rect, 0, RoundingPower::default()),
                    alpha: 200,
                },
            },
        ],
    };
    let damage = DamageRegion::from_rect(rect);
    let mut target = pair
        .vulkan
        .create_offscreen_target(2, 2)
        .expect("offscreen");
    pair.vulkan
        .render(
            &mut *target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage,
                preserve_contents: false,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: None,
            },
        )
        .expect("render the texture under the dim overlay");
    let mut read = [0_u8; 16];
    pair.vulkan
        .readback(&*target, rect, 8, &mut read)
        .expect("readback");
    assert_eq!(read[0], 0);
    assert!(read[1] > 0 && read[1] < 250, "dim should darken green, got {}", read[1]);
    assert!(read[3] > 200, "the opaque texture should remain under the overlay");
}

fn paint_elements(
    renderer: &mut dyn Renderer,
    elements: Vec<FrameElement>,
    width: i32,
    height: i32,
) -> Vec<u8> {
    let recipe = OutputFrameRecipe { elements };
    let damage = DamageRegion::from_rect(RectI::new(0, 0, width, height).expect("damage"));
    let mut target = renderer
        .create_offscreen_target(width, height)
        .expect("offscreen");
    renderer
        .render(
            &mut *target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage,
                preserve_contents: false,
                presentation_transform: OutputPresentationTransform::FlipY,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: None,
            },
        )
        .expect("render");
    let stride = width as usize * 4;
    let mut bytes = vec![0_u8; stride * height as usize];
    renderer
        .readback(&*target, RectI::new(0, 0, width, height).expect("read"), stride, &mut bytes)
        .expect("readback");
    bytes
}

fn shm(
    renderer: &mut dyn Renderer,
    bytes: &[u8],
    width: i32,
    height: i32,
) -> std::rc::Rc<dyn Texture> {
    renderer
        .import_shm(ShmImage {
            bytes,
            width,
            height,
            stride: width as usize * 4,
            fourcc: DrmFourcc::Argb8888,
        })
        .expect("shm")
}

fn hard_ring(outer: RectI, inner: RectI) -> RoundedRing {
    RoundedRing {
        outer: RootSurfaceClip::new(outer, 0, RoundingPower::default()),
        inner: RootSurfaceClip::new(inner, 0, RoundingPower::default()),
    }
}

#[test]
fn vulkan_matches_gles_dim_border_and_shadow() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan effect comparison: no matching DRM node");
        return;
    };
    let _ = pair.gbm;
    let green = solid([0, 255, 0, 255], 4, 2);
    let gl_green = shm(&mut *pair.gl, &green, 4, 2);
    let vk_green = shm(&mut *pair.vulkan, &green, 4, 2);
    let dim_clip =
        RootSurfaceClip::new(RectI::new(0, 0, 4, 2).expect("dim"), 0, RoundingPower::default());
    let dim = |texture| {
        vec![
            element(texture, 0, 0, 4, 2, u8::MAX, TextureMask::None),
            FrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::DimOverlay { clip: dim_clip, alpha: 128 },
            },
        ]
    };
    let gles_dim = paint_elements(&mut *pair.gl, dim(gl_green), 4, 2);
    assert!(gles_dim[1] < 250);
    assert_close(&gles_dim, &paint_elements(&mut *pair.vulkan, dim(vk_green), 4, 2), 1);

    let bounds = RectI::new(0, 0, 8, 8).expect("border");
    let inner = RectI::new(2, 2, 4, 4).expect("inner");
    let border = FrameElement::Effect {
        phase: FramePhase::OverWindow,
        element: EffectElement::WindowBorder {
            ring: hard_ring(bounds, inner),
            gradient: BorderGradient {
                start_color: RgbaColor::new(255, 255, 255, 255),
                end_color: RgbaColor::new(255, 255, 255, 255),
                angle_degrees: 0,
            },
            alpha: u8::MAX,
        },
    };
    let gles_border = paint_elements(&mut *pair.gl, vec![border.clone()], 8, 8);
    assert_eq!(&gles_border[..4], &[255, 255, 255, 255]);
    assert_eq!(&gles_border[4 * 8 * 4 + 4 * 4..4 * 8 * 4 + 4 * 4 + 4], &[0, 0, 0, 0]);
    assert_close(&gles_border, &paint_elements(&mut *pair.vulkan, vec![border], 8, 8), 1);

    let shadow = FrameElement::Effect {
        phase: FramePhase::UnderWindow,
        element: EffectElement::WindowShadow {
            clip: RootSurfaceClip::new(
                RectI::new(2, 2, 4, 4).expect("shadow"),
                0,
                RoundingPower::default(),
            ),
            style: ShadowVisualStyle {
                radius: 2,
                offset_x: 0,
                offset_y: 0,
                color: RgbaColor::new(0, 0, 0, 255),
                power: ShadowPower::default(),
            },
            alpha: u8::MAX,
        },
    };
    assert_close(
        &paint_elements(&mut *pair.gl, vec![shadow.clone()], 8, 8),
        &paint_elements(&mut *pair.vulkan, vec![shadow], 8, 8),
        2,
    );
}

#[test]
fn vulkan_matches_gles_blur_and_keeps_later_textures_on_top() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan blur comparison: no matching DRM node");
        return;
    };
    let _ = pair.gbm;
    let mut split = Vec::new();
    for y in 0..4 {
        for _x in 0..8 {
            if y < 2 {
                split.extend_from_slice(&[0, 255, 0, 255]);
            } else {
                split.extend_from_slice(&[255, 0, 0, 255]);
            }
        }
    }
    let blue = solid([255, 0, 0, 255], 1, 1);
    let gl_split = shm(&mut *pair.gl, &split, 8, 4);
    let vk_split = shm(&mut *pair.vulkan, &split, 8, 4);
    let gl_blue = shm(&mut *pair.gl, &blue, 1, 1);
    let vk_blue = shm(&mut *pair.vulkan, &blue, 1, 1);
    let scene = |backdrop, mark| {
        vec![
            element(backdrop, 0, 0, 8, 4, u8::MAX, TextureMask::None),
            FrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowBlurBackdrop {
                    ring: hard_ring(
                        RectI::new(0, 0, 8, 4).expect("outer"),
                        RectI::new(3, 1, 2, 2).expect("inner"),
                    ),
                    style: BlurVisualStyle { radius: 2, passes: 1 },
                    alpha: u8::MAX,
                },
            },
            element(mark, 0, 0, 1, 1, u8::MAX, TextureMask::None),
        ]
    };
    let gles = paint_elements(&mut *pair.gl, scene(gl_split, gl_blue), 8, 4);
    let vulkan = paint_elements(&mut *pair.vulkan, scene(vk_split, vk_blue), 8, 4);
    assert_eq!(&gles[..4], &[255, 0, 0, 255]);
    assert_close(&gles, &vulkan, 3);
}

#[test]
fn vulkan_capture_transparent_matches_gles() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan capture comparison: no matching DRM node");
        return;
    };
    let _ = pair.gbm;
    let green = solid([0, 255, 0, 255], 2, 2);
    let gl_green = shm(&mut *pair.gl, &green, 2, 2);
    let vk_green = shm(&mut *pair.vulkan, &green, 2, 2);
    let gl_captured = pair
        .gl
        .capture_transparent(
            &[element(gl_green, 0, 0, 2, 2, u8::MAX, TextureMask::None)],
            2,
            2,
            (2, 2),
            OutputPresentationTransform::FlipY,
        )
        .expect("gles capture");
    let vk_captured = pair
        .vulkan
        .capture_transparent(
            &[element(vk_green, 0, 0, 2, 2, u8::MAX, TextureMask::None)],
            2,
            2,
            (2, 2),
            OutputPresentationTransform::FlipY,
        )
        .expect("vulkan capture");
    let gles =
        paint(&mut *pair.gl, element(gl_captured, 0, 0, 2, 2, u8::MAX, TextureMask::None), 2, 2);
    assert_eq!(&gles[..4], &[0, 255, 0, 255]);
    assert_close(
        &gles,
        &paint(
            &mut *pair.vulkan,
            element(vk_captured, 0, 0, 2, 2, u8::MAX, TextureMask::None),
            2,
            2,
        ),
        1,
    );
}

#[test]
fn vulkan_preserves_untouched_scanout_pixels() {
    let Some(mut pair) = pair() else {
        eprintln!("skipping vulkan preserve comparison: no matching DRM node");
        return;
    };
    let modifiers = pair
        .vulkan
        .capabilities()
        .render_formats
        .iter()
        .find(|format| format.fourcc == DrmFourcc::Xrgb8888)
        .map(|format| format.modifiers.clone())
        .unwrap_or_default();
    let green = solid([0, 255, 0, 255], 8, 4);
    let texture = shm(&mut *pair.vulkan, &green, 8, 4);
    for index in 0..2 {
        let Some(bo) = modifiers.iter().find_map(|modifier| {
            pair.gbm.try_create_bo_with_modifiers(
                8,
                4,
                DrmFourcc::Xrgb8888,
                UseFlags::RENDERING | UseFlags::SCANOUT,
                &[crate::drm::Modifier(*modifier)],
            )
        }) else {
            eprintln!("skipping vulkan preserve comparison: GBM allocated no scanout buffer");
            return;
        };
        let dmabuf = bo.dmabuf().expect("scanout dma-buf");
        let mut target = pair
            .vulkan
            .import_output_target(dmabuf)
            .expect("import scanout");
        let recipe = OutputFrameRecipe {
            elements: vec![element(texture.clone(), 0, 0, 8, 4, u8::MAX, TextureMask::None)],
        };
        let full = DamageRegion::from_rect(RectI::new(0, 0, 8, 4).expect("full"));
        pair.vulkan
            .render(
                &mut *target,
                RenderFrame {
                    recipe: &recipe,
                    damage: &full,
                    preserve_contents: false,
                    presentation_transform: OutputPresentationTransform::FlipY,
                    sync_mode: RenderSyncMode::BlockingFinishFallback,
                    blur_reference: None,
                },
            )
            .expect("paint the scanout buffer");
        let empty = OutputFrameRecipe { elements: Vec::new() };
        let left = DamageRegion::from_rect(RectI::new(0, 0, 4, 4).expect("left"));
        pair.vulkan
            .render(
                &mut *target,
                RenderFrame {
                    recipe: &empty,
                    damage: &left,
                    preserve_contents: true,
                    presentation_transform: OutputPresentationTransform::FlipY,
                    sync_mode: RenderSyncMode::BlockingFinishFallback,
                    blur_reference: None,
                },
            )
            .expect("redraw the damaged half");
        let mut bytes = [0_u8; 8 * 4 * 4];
        pair.vulkan
            .readback(&*target, RectI::new(0, 0, 8, 4).expect("read"), 32, &mut bytes)
            .expect("read the scanout buffer");
        for y in 0..4 {
            for x in 0..8 {
                let start = y * 32 + x * 4;
                let pixel = &bytes[start..start + 4];
                if x < 4 {
                    assert_eq!(pixel, &[0, 0, 255, 255], "buffer {index} damaged {x},{y}");
                } else {
                    assert_eq!(pixel, &[0, 255, 0, 255], "buffer {index} preserved {x},{y}");
                }
            }
        }
    }
}
