use std::{
    any::Any,
    cell::RefCell,
    fs,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    rc::Rc,
};

use drm::buffer::DrmFourcc;

use crate::{
    dmabuf::{DmaBuf, DmaBufId, DmaBufPlane},
    drm::{Modifier, ARGB8888},
    region::{DamageRegion, RectI},
    render::{
        FrameElement, FramePhase, GraphicsApi, OffscreenTarget, OutputFrameRecipe,
        RenderCompletion, RenderFrame, RenderSyncMode, RenderTarget, Renderer,
        RendererCapabilities, RendererError, ShmImage, Texture, TextureElement, TextureMask,
        TextureSource,
    },
    sync::AcquireSync,
    transform::{Mat3, OutputPresentationTransform},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Call {
    MakeCurrent,
    ImportDmabuf,
    ImportShm,
    WaitAcquire,
    CreateOffscreen,
    Render,
    Readback,
}

#[derive(Debug)]
struct MockTexture;

impl Texture for MockTexture {}

#[derive(Debug)]
struct MockTarget {
    width: i32,
    height: i32,
}

impl RenderTarget for MockTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn width(&self) -> i32 {
        self.width
    }

    fn height(&self) -> i32 {
        self.height
    }
}

impl OffscreenTarget for MockTarget {}

#[derive(Debug)]
struct MockRenderer {
    calls: RefCell<Vec<Call>>,
    capabilities: RendererCapabilities,
}

impl Default for MockRenderer {
    fn default() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            capabilities: RendererCapabilities {
                sample_formats: Vec::new(),
                render_formats: Vec::new(),
                explicit_sync: false,
            },
        }
    }
}

impl MockRenderer {
    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    fn record(&self, call: Call) {
        self.calls.borrow_mut().push(call);
    }
}

impl Renderer for MockRenderer {
    fn api(&self) -> GraphicsApi {
        GraphicsApi::OpenGl
    }

    fn capabilities(&self) -> &RendererCapabilities {
        &self.capabilities
    }

    fn import_dmabuf(&mut self, _dmabuf: &DmaBuf) -> Result<Rc<dyn Texture>, RendererError> {
        self.record(Call::ImportDmabuf);
        Ok(Rc::new(MockTexture))
    }

    fn import_shm(&mut self, _image: ShmImage<'_>) -> Result<Rc<dyn Texture>, RendererError> {
        self.record(Call::ImportShm);
        Ok(Rc::new(MockTexture))
    }

    fn upload_rgba(
        &mut self,
        _pixels: &[u8],
        _width: i32,
        _height: i32,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        Ok(Rc::new(MockTexture))
    }

    fn import_output_target(
        &mut self,
        source: &DmaBuf,
    ) -> Result<Box<dyn RenderTarget>, RendererError> {
        Ok(Box::new(MockTarget { width: source.width, height: source.height }))
    }

    fn create_offscreen_target(
        &mut self,
        width: i32,
        height: i32,
    ) -> Result<Box<dyn OffscreenTarget>, RendererError> {
        self.record(Call::CreateOffscreen);
        Ok(Box::new(MockTarget { width, height }))
    }

    fn render(
        &mut self,
        _target: &mut dyn RenderTarget,
        _frame: RenderFrame<'_>,
    ) -> Result<RenderCompletion, RendererError> {
        self.record(Call::Render);
        Ok(RenderCompletion::blocking_fallback())
    }

    fn readback(
        &mut self,
        _target: &dyn RenderTarget,
        _rect: RectI,
        _stride: usize,
        dst: &mut [u8],
    ) -> Result<(), RendererError> {
        self.record(Call::Readback);
        dst.fill(0);
        Ok(())
    }

    fn capture_transparent(
        &mut self,
        _elements: &[FrameElement],
        _width: i32,
        _height: i32,
        _blur_reference: (i32, i32),
        _presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        Ok(Rc::new(MockTexture))
    }

    fn wait_acquire(&mut self, _acquire: &AcquireSync) -> Result<(), RendererError> {
        self.record(Call::WaitAcquire);
        Ok(())
    }

    fn make_current(&self) {
        self.record(Call::MakeCurrent);
    }

    fn wait_fence(&mut self, _fence: OwnedFd) -> Result<(), RendererError> {
        Ok(())
    }
}

fn sample_dmabuf() -> DmaBuf {
    let fd = OwnedFd::from(std::fs::File::open("/dev/null").expect("dev null"));
    DmaBuf {
        id: DmaBufId::next(),
        width: 1,
        height: 1,
        format: ARGB8888,
        modifier: Modifier::LINEAR,
        planes: Box::new([DmaBufPlane { offset: 0, stride: 4, fd }]),
    }
}

fn sample_shm() -> ShmImage<'static> {
    ShmImage {
        bytes: &[0, 0, 255, 255],
        width: 1,
        height: 1,
        stride: 4,
        fourcc: DrmFourcc::Argb8888,
    }
}

fn composite_imported_frame(renderer: &mut dyn Renderer) {
    renderer.make_current();
    let _client = renderer
        .import_dmabuf(&sample_dmabuf())
        .expect("dmabuf import");
    let texture = renderer.import_shm(sample_shm()).expect("shm import");
    renderer.wait_acquire(&AcquireSync::None).expect("acquire");
    let mut target = renderer.create_offscreen_target(1, 1).expect("offscreen");
    let rect = RectI::new(0, 0, 1, 1).expect("rect");
    let recipe = OutputFrameRecipe {
        elements: vec![FrameElement::Texture {
            phase: FramePhase::Content,
            element: TextureElement {
                source: TextureSource::Texture,
                texture,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                tex_transform: Mat3::identity(),
                damage: DamageRegion::from_rect(rect),
                blend: false,
                alpha: u8::MAX,
                mask: TextureMask::None,
            },
        }],
    };
    let damage = DamageRegion::from_rect(rect);
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
    let mut bytes = [0u8; 4];
    renderer
        .readback(&*target, rect, 4, &mut bytes)
        .expect("readback");
}

#[test]
fn compositing_waits_before_render_and_readback() {
    let mut renderer = MockRenderer::default();
    composite_imported_frame(&mut renderer);
    assert_eq!(
        renderer.calls(),
        vec![
            Call::MakeCurrent,
            Call::ImportDmabuf,
            Call::ImportShm,
            Call::WaitAcquire,
            Call::CreateOffscreen,
            Call::Render,
            Call::Readback,
        ]
    );
}

#[test]
fn direct_scanout_imports_without_waiting_or_rendering() {
    let mut renderer = MockRenderer::default();
    let _texture = renderer
        .import_dmabuf(&sample_dmabuf())
        .expect("dmabuf import");
    assert_eq!(renderer.calls(), vec![Call::ImportDmabuf]);
}

#[test]
fn gles_types_stay_inside_the_gles_module() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_sources(&root, &mut files);
    let banned = [
        concat!("khronos_types::", "gl"),
        concat!("Egl", "Context"),
        concat!("Egl", "Frame", "Buffer"),
        concat!("Gl", "Texture"),
        concat!("Offscreen", "Color", "Target"),
    ];
    let mut leaks = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).expect("read source");
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if banned.iter().any(|name| code.contains(name)) {
                leaks.push(format!("{}:{}: {line}", path.display(), index + 1));
            }
        }
    }
    assert!(leaks.is_empty(), "GL types leaked outside src/render/gles:\n{}", leaks.join("\n"));
}

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("read src");
    for entry in entries {
        let path = entry.expect("src entry").path();
        if path.is_dir() {
            if path.ends_with("gles")
                && path
                    .parent()
                    .is_some_and(|parent| parent.ends_with("render"))
            {
                continue;
            }
            collect_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
