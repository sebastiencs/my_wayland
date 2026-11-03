use std::{
    os::fd::{AsRawFd, OwnedFd},
    rc::Rc,
};

use khronos_types::{egl, gl};

use {
    super::{
        import::{gl_texture_name, GlTexture},
        offscreen::TextureFrameBuffer,
        readback::read_flip_y_framebuffer,
        resources::{
            BlurProgram, BorderProgram, EglFrameBuffer, GlesPrograms, ShadowProgram, TextureProgram,
        },
        sync::EGL_SYNC_NATIVE_FENCE_ANDROID,
        RenderItem, RenderItemMask,
    },
    crate::{
        drm::RenderSyncMode,
        region::{DamageRegion, RectI},
        render::{
            shadow_geometry, BlurVisualStyle, EffectElement, FrameElement, OutputFrameRecipe,
            RenderCompletion, RenderDeviceId, Texture, TextureSource,
        },
        state::RootSurfaceClip,
        transform::{Mat3, OutputPresentationTransform},
    },
};

pub(super) struct GlesDraw<'a> {
    pub(super) programs: &'a GlesPrograms,
    pub(super) device_id: RenderDeviceId,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl EglFrameBuffer {
    fn draw(&self) -> GlesDraw<'_> {
        GlesDraw {
            programs: &self.programs,
            device_id: self.device_id,
            width: self.width,
            height: self.height,
        }
    }

    pub fn clear(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            gl::ClearColor(1.0, 0.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
        }
        unsafe {
            gl::Flush();
        }
    }

    fn output_bounds(&self) -> Option<RectI> {
        RectI::new(0, 0, self.width, self.height)
    }

    fn clear_damage(
        &self,
        damage: &DamageRegion,
        preserve_contents: bool,
        output_transform: OutputPresentationTransform,
    ) {
        let Some(output_bounds) = self.output_bounds() else {
            return;
        };
        let mut clipped_damage = damage.clone();
        clipped_damage.intersect_rect(output_bounds);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            gl::ClearColor(1.0, 0.0, 0.0, 1.0);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
        }

        if !preserve_contents || clipped_damage.extents() == Some(output_bounds) {
            unsafe {
                gl::Disable(gl::SCISSOR_TEST as _);
                gl::Clear(gl::COLOR_BUFFER_BIT as _);
                gl::Flush();
            }
            return;
        }

        unsafe {
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    scissor_y(self.height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::Clear(gl::COLOR_BUFFER_BIT as _);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::Flush();
        }
    }

    pub fn clear_with(&self, _r: f32, _g: f32, _b: f32, _a: f32) {
        // let ops = self.take_render_ops();
        // self.render(ops, Some(&Color { r, g, b, a }));
    }

    fn finish_render(&self, render_sync_mode: RenderSyncMode) -> RenderCompletion {
        match render_sync_mode {
            RenderSyncMode::ExplicitInFence => match self.export_native_render_fence() {
                Ok(in_fence_fd) => {
                    if crate::logger::ENABLE_NOISY_LOG {
                        log::debug!(
                            "egl finish_render: exported explicit render fence fd={}",
                            in_fence_fd.as_raw_fd()
                        );
                    }
                    RenderCompletion::explicit(in_fence_fd)
                }
                Err(error) => {
                    log::warn!(
                        "egl finish_render: explicit fence export failed, falling back to glFinish: {:?}",
                        error
                    );
                    unsafe { gl::Finish() };
                    RenderCompletion::blocking_fallback()
                }
            },
            RenderSyncMode::BlockingFinishFallback => {
                unsafe { gl::Finish() };
                RenderCompletion::blocking_fallback()
            }
        }
    }

    fn export_native_render_fence(&self) -> Result<OwnedFd, egl::Error> {
        let sync =
            self.egl
                .create_sync_khr(self.display, EGL_SYNC_NATIVE_FENCE_ANDROID, &[egl::NONE])?;
        unsafe { gl::Flush() };
        let fence_fd = self.egl.dup_native_fence_fd_android(self.display, sync);
        let destroy_result = self.egl.destroy_sync_khr(self.display, sync);
        if let Err(error) = destroy_result {
            log::warn!("egl destroy_sync_khr failed after fence export: {:?}", error);
        }
        fence_fd
    }

    pub fn render(
        &self,
        recipe: &OutputFrameRecipe,
        projection: Mat3,
        render_sync_mode: RenderSyncMode,
        damage: &DamageRegion,
        preserve_contents: bool,
        output_transform: OutputPresentationTransform,
    ) -> RenderCompletion {
        self.clear_damage(damage, preserve_contents, output_transform);

        if recipe.is_empty() {
            return self.finish_render(render_sync_mode);
        }

        self.draw().render_frame_elements_to_target(
            self.framebuffer_object,
            self.width,
            self.height,
            &recipe.elements,
            projection,
            damage,
            output_transform,
        );

        self.finish_render(render_sync_mode)
    }
}

impl GlesDraw<'_> {
    fn texture_program(&self) -> Result<&TextureProgram, &str> {
        self.programs
            .texture
            .get_or_init(create_texture_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn blur_program(&self) -> Result<&BlurProgram, &str> {
        self.programs
            .blur
            .get_or_init(create_blur_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn border_program(&self) -> Result<&BorderProgram, &str> {
        self.programs
            .border
            .get_or_init(create_border_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn shadow_program(&self) -> Result<&ShadowProgram, &str> {
        self.programs
            .shadow
            .get_or_init(create_shadow_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn dim_texture(&self) -> Option<Rc<dyn Texture>> {
        if let Some(texture) = self.programs.dim.get() {
            return Some(Rc::clone(texture) as Rc<dyn Texture>);
        }
        let texture = GlTexture::from_rgba(&[0, 0, 0, u8::MAX], 1, 1, self.device_id)
            .ok()
            .map(Rc::new)?;
        let _ = self.programs.dim.set(Rc::clone(&texture));
        Some(texture)
    }

    fn borrowed_texture(&self, name: gl::Uint) -> Rc<dyn Texture> {
        Rc::new(GlTexture {
            image: None,
            texture: name,
            delete_on_drop: false,
            device_id: self.device_id,
        })
    }
    fn render_texture_elements_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        items: &[RenderItem],
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
    ) {
        if items.is_empty() {
            return;
        }
        let program = match self.texture_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("texture shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 16] =
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let stride = (4 * std::mem::size_of::<f32>()) as i32;
        let output_bounds = RectI::new(0, 0, target_width, target_height);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::Uniform1i(program.sampler_location, 0);
            gl::ActiveTexture(gl::TEXTURE0 as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::EnableVertexAttribArray(program.texcoord_location);
        }

        for item in items {
            let Some(item_bounds) = RectI::new(item.x, item.y, item.width, item.height) else {
                continue;
            };
            let mut clipped_damage = damage.clone();
            if let Some(output_bounds) = output_bounds {
                clipped_damage.intersect_rect(output_bounds);
            }
            clipped_damage.intersect_rect(item_bounds);
            clipped_damage.intersect_rect(item.damage.extents().unwrap_or(item_bounds));
            if clipped_damage.is_empty() {
                continue;
            }
            let transform = crate::transform::project_box(
                item.x as f32,
                item.y as f32,
                item.width as f32,
                item.height as f32,
                projection,
            )
            .to_gl_array();
            let tex_transform = item.tex_transform.to_gl_array();
            unsafe {
                if item.blend || item.alpha < u8::MAX {
                    gl::Enable(gl::BLEND as _);
                } else {
                    gl::Disable(gl::BLEND as _);
                }
                let Some(texture_name) = gl_texture_name(item.texture.as_ref(), self.device_id)
                else {
                    continue;
                };
                gl::BindTexture(gl::TEXTURE_2D as _, texture_name);
                gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
                gl::UniformMatrix3fv(program.tex_transform_location, 1, 0, tex_transform.as_ptr());
                gl::Uniform2f(program.item_size_location, item.width as f32, item.height as f32);
                gl::Uniform1f(program.alpha_location, f32::from(item.alpha) / 255.0);
                match &item.mask {
                    RenderItemMask::None => {
                        gl::Uniform1i(program.clip_mode_location, 0);
                    }
                    RenderItemMask::RoundedClip { clip } => {
                        gl::Uniform1i(program.clip_mode_location, 1);
                        gl::Uniform4f(
                            program.clip_outer_rect_location,
                            clip.outer_rect.x as f32,
                            clip.outer_rect.y as f32,
                            clip.outer_rect.width as f32,
                            clip.outer_rect.height as f32,
                        );
                        gl::Uniform4f(program.clip_inner_rect_location, 0.0, 0.0, 0.0, 0.0);
                        gl::Uniform1f(
                            program.outer_corner_radius_location,
                            clip.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.outer_rounding_power_location,
                            clip.rounding_power.get().max(0.0001),
                        );
                        gl::Uniform1f(program.inner_corner_radius_location, 0.0);
                        gl::Uniform1f(program.inner_rounding_power_location, 0.0001);
                    }
                    RenderItemMask::FrameRing { ring } => {
                        gl::Uniform1i(program.clip_mode_location, 2);
                        gl::Uniform4f(
                            program.clip_outer_rect_location,
                            ring.outer.outer_rect.x as f32,
                            ring.outer.outer_rect.y as f32,
                            ring.outer.outer_rect.width as f32,
                            ring.outer.outer_rect.height as f32,
                        );
                        gl::Uniform4f(
                            program.clip_inner_rect_location,
                            ring.inner.outer_rect.x as f32,
                            ring.inner.outer_rect.y as f32,
                            ring.inner.outer_rect.width as f32,
                            ring.inner.outer_rect.height as f32,
                        );
                        gl::Uniform1f(
                            program.outer_corner_radius_location,
                            ring.outer.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.outer_rounding_power_location,
                            ring.outer.rounding_power.get().max(0.0001),
                        );
                        gl::Uniform1f(
                            program.inner_corner_radius_location,
                            ring.inner.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.inner_rounding_power_location,
                            ring.inner.rounding_power.get().max(0.0001),
                        );
                    }
                }
                gl::VertexAttribPointer(
                    program.position_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().cast(),
                );
                gl::VertexAttribPointer(
                    program.texcoord_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().add(2).cast(),
                );
                gl::Enable(gl::SCISSOR_TEST as _);
                for rect in clipped_damage.rects() {
                    gl::Scissor(
                        rect.x,
                        scissor_y(target_height, *rect, output_transform),
                        rect.width,
                        rect.height,
                    );
                    gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
                }
                gl::Disable(gl::SCISSOR_TEST as _);
            }
        }

        unsafe {
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::Disable(gl::BLEND as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::DisableVertexAttribArray(program.texcoord_location);
            gl::UseProgram(0);
        }
    }

    fn blur_snapshot_bounds(
        &self,
        clip: crate::state::RootSurfaceClip,
        blur_style: BlurVisualStyle,
    ) -> Option<RectI> {
        let output_bounds = RectI::new(0, 0, self.width, self.height)?;
        let padding = blur_style
            .radius
            .max(1)
            .saturating_mul(i32::from(blur_style.passes.max(1)))
            .saturating_mul(4);
        RectI::from_extents(
            clip.outer_rect.x.saturating_sub(padding),
            clip.outer_rect.y.saturating_sub(padding),
            clip.outer_rect.x2().saturating_add(padding),
            clip.outer_rect.y2().saturating_add(padding),
        )
        .and_then(|bounds| bounds.intersection(output_bounds))
    }

    fn build_blurred_backdrop(
        &self,
        framebuffer_object: gl::GLuint,
        target_height: i32,
        ring: crate::render::RoundedRing,
        blur_style: BlurVisualStyle,
        output_transform: OutputPresentationTransform,
    ) -> Option<(TextureFrameBuffer, RectI)> {
        let blur_bounds = self.blur_snapshot_bounds(ring.outer, blur_style)?;
        let copy_rect = framebuffer_rect_for_target(target_height, blur_bounds, output_transform)?;
        let blur_input = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::BindTexture(gl::TEXTURE_2D as _, blur_input.texture);
            gl::CopyTexSubImage2D(
                gl::TEXTURE_2D as _,
                0,
                0,
                0,
                copy_rect.x,
                copy_rect.y,
                copy_rect.width,
                copy_rect.height,
            );
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
        }
        let mut scratch_a = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        let mut scratch_b = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        self.blur_texture(
            blur_input.texture,
            &mut scratch_a,
            &mut scratch_b,
            blur_bounds.width,
            blur_bounds.height,
            blur_style.radius.max(1) as f32,
            blur_style.passes.max(1),
            crate::transform::output_projection(
                blur_bounds.width as f32,
                blur_bounds.height as f32,
                output_transform,
            ),
        );
        Some((scratch_a, blur_bounds))
    }

    fn blur_texture(
        &self,
        input_texture: gl::GLuint,
        scratch_a: &mut TextureFrameBuffer,
        scratch_b: &mut TextureFrameBuffer,
        width: i32,
        height: i32,
        radius: f32,
        passes: u8,
        projection: Mat3,
    ) {
        let program = match self.blur_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("blur shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 16] =
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let stride = (4 * std::mem::size_of::<f32>()) as i32;
        let transform =
            crate::transform::project_box(0.0, 0.0, width as f32, height as f32, projection)
                .to_gl_array();
        let tex_transform = Mat3::identity().to_gl_array();
        let sanitized_radius = radius.max(1.0);
        let pass_draws =
            blur_pass_draws(input_texture, scratch_a.texture, scratch_b.texture, passes);
        unsafe {
            gl::UseProgram(program.program);
            gl::Uniform1i(program.sampler_location, 0);
            gl::Uniform2f(program.texel_size_location, 1.0 / width as f32, 1.0 / height as f32);
            gl::Uniform1f(program.radius_location, sanitized_radius);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::UniformMatrix3fv(program.tex_transform_location, 1, 0, tex_transform.as_ptr());
            gl::Uniform2f(program.item_size_location, width as f32, height as f32);
            gl::EnableVertexAttribArray(program.position_location);
            gl::EnableVertexAttribArray(program.texcoord_location);
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::VertexAttribPointer(
                program.texcoord_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().add(2).cast(),
            );
            gl::ActiveTexture(gl::TEXTURE0 as _);
        }
        for (source_texture, target_texture, direction) in &pass_draws {
            let target = if *target_texture == scratch_a.texture {
                scratch_a.framebuffer_object
            } else {
                scratch_b.framebuffer_object
            };
            unsafe {
                gl::BindFramebuffer(gl::FRAMEBUFFER as _, target);
                gl::Viewport(0, 0, width, height);
                gl::Uniform2f(program.direction_location, direction[0], direction[1]);
                gl::BindTexture(gl::TEXTURE_2D as _, *source_texture);
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
        }
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::DisableVertexAttribArray(program.position_location);
            gl::DisableVertexAttribArray(program.texcoord_location);
            gl::UseProgram(0);
        }
        if pass_draws
            .last()
            .is_some_and(|(_, target_texture, _)| *target_texture == scratch_b.texture)
        {
            std::mem::swap(scratch_a, scratch_b);
        }
    }
}

fn blur_pass_draws(
    input_texture: gl::GLuint,
    scratch_a_texture: gl::GLuint,
    scratch_b_texture: gl::GLuint,
    passes: u8,
) -> Vec<(gl::GLuint, gl::GLuint, [f32; 2])> {
    let mut draws = Vec::with_capacity(usize::from(passes.max(1)) * 2);
    let mut source_texture = input_texture;
    for _ in 0..passes.max(1) {
        for (target_texture, direction) in
            [(scratch_a_texture, [1.0f32, 0.0f32]), (scratch_b_texture, [0.0f32, 1.0f32])]
        {
            draws.push((source_texture, target_texture, direction));
            source_texture = target_texture;
        }
    }
    draws
}

impl EglFrameBuffer {
    /// Draws `elements` into a new transparent texture. Coordinates are local to that texture.
    pub(crate) fn capture_transparent(
        &self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        presentation_transform: OutputPresentationTransform,
    ) -> Option<GlTexture> {
        self.draw()
            .capture_transparent(elements, width, height, presentation_transform)
    }

    /// Draw a window into another framebuffer and clear that target to transparent first.
    ///
    /// The output framebuffer keeps its own pixels. FlipY matches [`EglFrameBuffer::read_top_left`].
    pub(crate) fn render_into_transparent(
        &self,
        framebuffer: gl::GLuint,
        width: i32,
        height: i32,
        recipe: &OutputFrameRecipe,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer);
            gl::Viewport(0, 0, width, height);
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::ClearColor(0.0, 0.0, 0.0, 0.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
        }
        let Some(damage) = RectI::new(0, 0, width, height).map(DamageRegion::from_rect) else {
            return;
        };
        self.draw().render_frame_elements_to_target(
            framebuffer,
            width,
            height,
            &recipe.elements,
            crate::transform::output_projection(
                width as f32,
                height as f32,
                OutputPresentationTransform::FlipY,
            ),
            &damage,
            OutputPresentationTransform::FlipY,
        );
    }

    pub(crate) fn read_top_left(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        dst_stride: usize,
        dst: &mut [u8],
    ) -> bool {
        read_flip_y_framebuffer(self.framebuffer_object, x, y, width, height, dst_stride, dst)
    }
}

impl GlesDraw<'_> {
    pub(super) fn capture_transparent(
        &self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        presentation_transform: OutputPresentationTransform,
    ) -> Option<GlTexture> {
        let framebuffer = TextureFrameBuffer::new(width, height)?;
        let damage = DamageRegion::from_rect(RectI::new(0, 0, width, height)?);
        let projection = crate::transform::output_projection(
            width as f32,
            height as f32,
            presentation_transform,
        );
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer.framebuffer_object);
            gl::Viewport(0, 0, width, height);
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::ClearColor(0.0, 0.0, 0.0, 0.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
        }
        self.render_frame_elements_to_target(
            framebuffer.framebuffer_object,
            width,
            height,
            elements,
            projection,
            &damage,
            presentation_transform,
        );
        Some(framebuffer.into_texture(self.device_id))
    }

    fn render_frame_elements_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        elements: &[FrameElement],
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
    ) {
        let mut next_index = 0usize;
        while next_index < elements.len() {
            match &elements[next_index] {
                FrameElement::Texture { .. } => {
                    let batch_start = next_index;
                    next_index += 1;
                    while next_index < elements.len()
                        && matches!(elements[next_index], FrameElement::Texture { .. })
                    {
                        next_index += 1;
                    }
                    let batch: Vec<_> = elements[batch_start..next_index]
                        .iter()
                        .filter_map(|element| match element {
                            FrameElement::Texture { element, .. } => Some(element.clone()),
                            FrameElement::Effect { .. } => None,
                        })
                        .collect();
                    self.render_texture_elements_to_target(
                        framebuffer_object,
                        target_width,
                        target_height,
                        &batch,
                        projection,
                        damage,
                        output_transform,
                    );
                }
                FrameElement::Effect { element: effect, .. } => {
                    next_index += 1;
                    match effect {
                        EffectElement::WindowBlurBackdrop { ring, style, alpha } => {
                            let Some((texture, blur_bounds)) = self.build_blurred_backdrop(
                                framebuffer_object,
                                target_height,
                                *ring,
                                *style,
                                output_transform,
                            ) else {
                                continue;
                            };
                            let item = RenderItem {
                                source: TextureSource::Texture,
                                texture: self.borrowed_texture(texture.texture),
                                x: blur_bounds.x,
                                y: blur_bounds.y,
                                width: blur_bounds.width,
                                height: blur_bounds.height,
                                tex_transform: Mat3::identity(),
                                damage: damage.clone(),
                                blend: true,
                                alpha: *alpha,
                                mask: RenderItemMask::FrameRing {
                                    ring: crate::render::RoundedRing {
                                        outer: RootSurfaceClip::new(
                                            ring.outer.outer_rect.translate(
                                                blur_bounds.x.saturating_neg(),
                                                blur_bounds.y.saturating_neg(),
                                            ),
                                            ring.outer.corner_radius,
                                            ring.outer.rounding_power,
                                        ),
                                        inner: RootSurfaceClip::new(
                                            ring.inner.outer_rect.translate(
                                                blur_bounds.x.saturating_neg(),
                                                blur_bounds.y.saturating_neg(),
                                            ),
                                            ring.inner.corner_radius,
                                            ring.inner.rounding_power,
                                        ),
                                    },
                                },
                            };
                            self.render_texture_elements_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                &[item],
                                projection,
                                damage,
                                output_transform,
                            );
                        }
                        EffectElement::WindowBorder { ring, gradient, alpha } => {
                            self.render_border_element_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                projection,
                                damage,
                                output_transform,
                                *ring,
                                *gradient,
                                *alpha,
                            );
                        }
                        EffectElement::WindowShadow { clip, style, alpha } => {
                            self.render_shadow_element_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                projection,
                                damage,
                                output_transform,
                                *clip,
                                *style,
                                *alpha,
                            );
                        }
                        EffectElement::DimOverlay { clip, alpha } => {
                            let Some(texture) = self.dim_texture() else {
                                continue;
                            };
                            let item = RenderItem {
                                source: TextureSource::Texture,
                                texture,
                                x: clip.outer_rect.x,
                                y: clip.outer_rect.y,
                                width: clip.outer_rect.width,
                                height: clip.outer_rect.height,
                                tex_transform: Mat3::identity(),
                                damage: damage.clone(),
                                blend: true,
                                alpha: *alpha,
                                mask: RenderItemMask::RoundedClip {
                                    clip: RootSurfaceClip::new(
                                        RectI::new(
                                            0,
                                            0,
                                            clip.outer_rect.width,
                                            clip.outer_rect.height,
                                        )
                                        .expect("dim overlay bounds should stay valid"),
                                        clip.corner_radius,
                                        clip.rounding_power,
                                    ),
                                },
                            };
                            self.render_texture_elements_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                &[item],
                                projection,
                                damage,
                                output_transform,
                            );
                        }
                    }
                }
            }
        }
    }

    fn render_border_element_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
        ring: crate::render::RoundedRing,
        gradient: crate::render::BorderGradient,
        alpha: u8,
    ) {
        let bounds = ring.outer.outer_rect;
        let mut clipped_damage = damage.clone();
        let Some(output_bounds) = RectI::new(0, 0, target_width, target_height) else {
            return;
        };
        clipped_damage.intersect_rect(output_bounds);
        clipped_damage.intersect_rect(bounds);
        if clipped_damage.is_empty() {
            return;
        }

        let program = match self.border_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("border shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let stride = (2 * std::mem::size_of::<f32>()) as i32;
        let transform = crate::transform::project_box(
            bounds.x as f32,
            bounds.y as f32,
            bounds.width as f32,
            bounds.height as f32,
            projection,
        )
        .to_gl_array();
        let start = gradient.start_color.to_premultiplied_rgba_bytes();
        let end = gradient.end_color.to_premultiplied_rgba_bytes();

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::Enable(gl::BLEND as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::Uniform2f(program.item_size_location, bounds.width as f32, bounds.height as f32);
            gl::Uniform4f(
                program.outer_rect_location,
                0.0,
                0.0,
                bounds.width as f32,
                bounds.height as f32,
            );
            gl::Uniform4f(
                program.inner_rect_location,
                ring.inner.outer_rect.x.saturating_sub(bounds.x) as f32,
                ring.inner.outer_rect.y.saturating_sub(bounds.y) as f32,
                ring.inner.outer_rect.width as f32,
                ring.inner.outer_rect.height as f32,
            );
            gl::Uniform1f(
                program.outer_corner_radius_location,
                ring.outer.corner_radius.max(0) as f32,
            );
            gl::Uniform1f(
                program.outer_rounding_power_location,
                ring.outer.rounding_power.get().max(0.0001),
            );
            gl::Uniform1f(
                program.inner_corner_radius_location,
                ring.inner.corner_radius.max(0) as f32,
            );
            gl::Uniform1f(
                program.inner_rounding_power_location,
                ring.inner.rounding_power.get().max(0.0001),
            );
            gl::Uniform4f(
                program.start_color_location,
                f32::from(start[0]) / 255.0,
                f32::from(start[1]) / 255.0,
                f32::from(start[2]) / 255.0,
                f32::from(start[3]) / 255.0,
            );
            gl::Uniform4f(
                program.end_color_location,
                f32::from(end[0]) / 255.0,
                f32::from(end[1]) / 255.0,
                f32::from(end[2]) / 255.0,
                f32::from(end[3]) / 255.0,
            );
            gl::Uniform1f(program.angle_location, gradient.angle_degrees as f32);
            gl::Uniform1f(program.alpha_location, f32::from(alpha) / 255.0);
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    scissor_y(target_height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::Disable(gl::BLEND as _);
            gl::UseProgram(0);
        }
    }

    fn render_shadow_element_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
        clip: crate::state::RootSurfaceClip,
        style: crate::render::ShadowVisualStyle,
        alpha: u8,
    ) {
        let Some(shadow_geometry) = shadow_geometry(clip, style) else {
            return;
        };
        let bounds = shadow_geometry.bounds;

        let mut clipped_damage = damage.clone();
        let Some(output_bounds) = RectI::new(0, 0, target_width, target_height) else {
            return;
        };
        clipped_damage.intersect_rect(output_bounds);
        clipped_damage.intersect_rect(bounds);
        if clipped_damage.is_empty() {
            return;
        }

        let program = match self.shadow_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("shadow shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let stride = (2 * std::mem::size_of::<f32>()) as i32;
        let transform = crate::transform::project_box(
            bounds.x as f32,
            bounds.y as f32,
            bounds.width as f32,
            bounds.height as f32,
            projection,
        )
        .to_gl_array();
        let color = shadow_color_with_alpha(style.color, alpha);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::Enable(gl::BLEND as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::Uniform2f(program.item_size_location, bounds.width as f32, bounds.height as f32);
            gl::Uniform4f(program.shadow_color_location, color[0], color[1], color[2], color[3]);
            gl::Uniform4f(
                program.inner_rect_location,
                shadow_geometry.inner_rect.x as f32,
                shadow_geometry.inner_rect.y as f32,
                shadow_geometry.inner_rect.width as f32,
                shadow_geometry.inner_rect.height as f32,
            );
            gl::Uniform1f(program.corner_radius_location, clip.corner_radius.max(0) as f32);
            gl::Uniform1f(program.rounding_power_location, clip.rounding_power.get().max(0.0001));
            gl::Uniform1f(program.shadow_range_location, shadow_geometry.shadow_range as f32);
            gl::Uniform1f(program.shadow_power_location, style.power.get());
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    scissor_y(target_height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::Disable(gl::BLEND as _);
            gl::UseProgram(0);
        }
    }
}

fn create_texture_program() -> Result<TextureProgram, String> {
    let vertex_source = include_str!("../../shaders/texture_clip.vert");
    let fragment_source = include_str!("../../shaders/texture_clip.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "texture program")?;
    Ok(TextureProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        texcoord_location: unsafe {
            gl::GetAttribLocation(program, b"a_texcoord\0".as_ptr().cast()) as gl::Uint
        },
        sampler_location: unsafe {
            gl::GetUniformLocation(program, b"u_texture\0".as_ptr().cast())
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        tex_transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_tex_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        clip_mode_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_mode\0".as_ptr().cast())
        },
        clip_outer_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_rect\0".as_ptr().cast())
        },
        clip_inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_rect\0".as_ptr().cast())
        },
        outer_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_corner_radius\0".as_ptr().cast())
        },
        outer_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_rounding_power\0".as_ptr().cast())
        },
        inner_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_corner_radius\0".as_ptr().cast())
        },
        inner_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_rounding_power\0".as_ptr().cast())
        },
        alpha_location: unsafe { gl::GetUniformLocation(program, b"u_alpha\0".as_ptr().cast()) },
    })
}

fn create_border_program() -> Result<BorderProgram, String> {
    let vertex_source = include_str!("../../shaders/shadow.vert");
    let fragment_source = include_str!("../../shaders/border.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "border program")?;

    Ok(BorderProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        outer_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_rect\0".as_ptr().cast())
        },
        inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rect\0".as_ptr().cast())
        },
        outer_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_corner_radius\0".as_ptr().cast())
        },
        outer_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_rounding_power\0".as_ptr().cast())
        },
        inner_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_corner_radius\0".as_ptr().cast())
        },
        inner_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rounding_power\0".as_ptr().cast())
        },
        start_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_start\0".as_ptr().cast())
        },
        end_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_end\0".as_ptr().cast())
        },
        angle_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_angle_degrees\0".as_ptr().cast())
        },
        alpha_location: unsafe { gl::GetUniformLocation(program, b"u_alpha\0".as_ptr().cast()) },
    })
}

fn create_shadow_program() -> Result<ShadowProgram, String> {
    let vertex_source = include_str!("../../shaders/shadow.vert");
    let fragment_source = include_str!("../../shaders/shadow.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "shadow program")?;

    Ok(ShadowProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        shadow_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_color\0".as_ptr().cast())
        },
        inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rect\0".as_ptr().cast())
        },
        corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_corner_radius\0".as_ptr().cast())
        },
        rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_rounding_power\0".as_ptr().cast())
        },
        shadow_range_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_range\0".as_ptr().cast())
        },
        shadow_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_power\0".as_ptr().cast())
        },
    })
}

fn clear_framebuffer(framebuffer_object: gl::GLuint, width: i32, height: i32) {
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
        gl::Viewport(0, 0, width, height);
        gl::Disable(gl::SCISSOR_TEST as _);
        gl::ClearColor(0.0, 0.0, 0.0, 0.0);
        gl::Clear(gl::COLOR_BUFFER_BIT as _);
        gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
    }
}

fn create_blur_program() -> Result<BlurProgram, String> {
    let vertex_source = include_str!("../../shaders/blur.vert");
    let fragment_source = include_str!("../../shaders/blur.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "blur program")?;
    Ok(BlurProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        texcoord_location: unsafe {
            gl::GetAttribLocation(program, b"a_texcoord\0".as_ptr().cast()) as gl::Uint
        },
        sampler_location: unsafe {
            gl::GetUniformLocation(program, b"u_texture\0".as_ptr().cast())
        },
        direction_location: unsafe {
            gl::GetUniformLocation(program, b"u_direction\0".as_ptr().cast())
        },
        texel_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_texel_size\0".as_ptr().cast())
        },
        radius_location: unsafe { gl::GetUniformLocation(program, b"u_radius\0".as_ptr().cast()) },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        tex_transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_tex_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
    })
}

fn compile_shader(kind: gl::Enum, source: &str) -> Result<gl::Uint, String> {
    let shader = unsafe { gl::CreateShader(kind) };
    // `string` and `length` are parallel arrays of `count` elements (here count == 1).
    let strings: [*const gl::GLchar; 1] = [source.as_ptr().cast::<gl::GLchar>()];
    let lengths: [gl::GLint; 1] = [source.len() as gl::GLint];

    unsafe {
        gl::ShaderSource(shader, 1, strings.as_ptr(), lengths.as_ptr());
        gl::CompileShader(shader);
    }

    let mut status = 0;
    unsafe { gl::GetShaderiv(shader, gl::COMPILE_STATUS as _, &mut status) };
    if status == 0 {
        unsafe { gl::DeleteShader(shader) };
        return Err("could not compile shader".into());
    }

    Ok(shader)
}

fn link_program(
    vertex_shader: gl::Uint,
    fragment_shader: gl::Uint,
    label: &str,
) -> Result<gl::Uint, String> {
    let program = unsafe { gl::CreateProgram() };
    unsafe {
        gl::AttachShader(program, vertex_shader);
        gl::AttachShader(program, fragment_shader);
        gl::LinkProgram(program);
        gl::DeleteShader(vertex_shader);
        gl::DeleteShader(fragment_shader);
    }

    let mut status = 0;
    unsafe { gl::GetProgramiv(program, gl::LINK_STATUS as _, &mut status) };
    if status == 0 {
        unsafe { gl::DeleteProgram(program) };
        return Err(format!("could not link {label}"));
    }
    Ok(program)
}

fn scissor_y(
    target_height: i32,
    rect: RectI,
    output_transform: OutputPresentationTransform,
) -> i32 {
    framebuffer_rect_for_target(target_height, rect, output_transform)
        .map(|framebuffer_rect| framebuffer_rect.y)
        .unwrap_or(0)
}

fn framebuffer_rect_for_target(
    target_height: i32,
    rect: RectI,
    output_transform: OutputPresentationTransform,
) -> Option<RectI> {
    RectI::new(
        rect.x,
        match output_transform {
            OutputPresentationTransform::Normal => target_height - rect.y - rect.height,
            OutputPresentationTransform::FlipY => rect.y,
        },
        rect.width,
        rect.height,
    )
}

fn shadow_color_with_alpha(color: crate::config::RgbaColor, alpha: u8) -> [f32; 4] {
    let premultiplied = color.to_premultiplied_rgba_bytes();
    let global_alpha = f32::from(alpha) / 255.0;
    [
        (f32::from(premultiplied[0]) / 255.0) * global_alpha,
        (f32::from(premultiplied[1]) / 255.0) * global_alpha,
        (f32::from(premultiplied[2]) / 255.0) * global_alpha,
        (f32::from(premultiplied[3]) / 255.0) * global_alpha,
    ]
}

#[cfg(test)]
mod tests {
    use {
        super::{blur_pass_draws, framebuffer_rect_for_target, shadow_color_with_alpha},
        crate::{config::RgbaColor, region::RectI, transform::OutputPresentationTransform},
    };

    #[test]
    fn blur_pass_draws_ping_pongs_previous_result_across_multiple_passes() {
        let draws = blur_pass_draws(10, 20, 30, 2);

        assert_eq!(
            draws,
            vec![
                (10, 20, [1.0, 0.0]),
                (20, 30, [0.0, 1.0]),
                (30, 20, [1.0, 0.0]),
                (20, 30, [0.0, 1.0]),
            ]
        );
    }

    #[test]
    fn blur_shader_uses_radius_in_sample_offsets() {
        let source = include_str!("../../shaders/blur.frag");

        assert!(source.contains("1.384615 * radius"));
        assert!(source.contains("3.230769 * radius"));
    }

    #[test]
    fn blur_backdrop_is_built_from_in_stream_framebuffer_snapshot() {
        let source = include_str!("pipeline.rs");

        assert!(source.contains("self.build_blurred_backdrop("));
        assert!(source.contains("gl::CopyTexSubImage2D("));
        assert!(source
            .contains("framebuffer_rect_for_target(target_height, blur_bounds, output_transform)"));
    }

    #[test]
    fn framebuffer_rect_helper_uses_the_active_render_target_height() {
        let rect = RectI::new(10, 20, 30, 40).unwrap();

        assert_eq!(
            framebuffer_rect_for_target(100, rect, OutputPresentationTransform::Normal),
            RectI::new(10, 40, 30, 40)
        );
        assert_eq!(
            framebuffer_rect_for_target(100, rect, OutputPresentationTransform::FlipY),
            RectI::new(10, 20, 30, 40)
        );
    }

    #[test]
    fn texture_and_border_shaders_use_rounded_inner_cutouts_without_texture_dim() {
        let texture_source = include_str!("../../shaders/texture_clip.frag");
        let border_source = include_str!("../../shaders/border.frag");

        assert!(texture_source.contains("pointInRoundedRect(point, u_clip_inner_rect"));
        assert!(!texture_source.contains("u_dim_amount"));
        assert!(!texture_source.contains("color.rgb *= (1.0 - u_dim_amount)"));
        assert!(border_source.contains("pointInRoundedRect(v_local_position, u_inner_rect"));
        assert!(!border_source.contains("pointInRect(v_local_position, u_inner_rect)"));
    }

    #[test]
    fn shadow_alpha_scales_premultiplied_rgb_and_alpha() {
        let color = shadow_color_with_alpha(RgbaColor::new(255, 128, 0, 128), 128);

        assert!((color[0] - 0.25196466).abs() < 0.0001);
        assert!((color[1] - 0.12598233).abs() < 0.0001);
        assert_eq!(color[2], 0.0);
        assert!((color[3] - 0.25196466).abs() < 0.0001);
    }

    #[test]
    fn gl_texture_name_rejects_another_backend_and_another_drm_device() {
        let device_id = crate::render::RenderDeviceId::next();
        let texture = super::super::import::GlTexture {
            image: None,
            texture: 7,
            delete_on_drop: false,
            device_id,
        };

        assert_eq!(super::super::import::gl_texture_name(&texture, device_id), Some(7));
        assert!(super::super::import::gl_texture_name(
            &texture,
            crate::render::RenderDeviceId::next()
        )
        .is_none());
        assert!(super::super::import::gl_texture_name(
            crate::render::inert_texture().as_ref(),
            device_id
        )
        .is_none());
    }
}
