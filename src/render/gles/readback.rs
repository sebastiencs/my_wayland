use khronos_types::gl;

/// Copy a FlipY framebuffer into top-left rows.
///
/// `y` is the framebuffer y of the visual top, matching [`framebuffer_rect_for_target`] for
/// [`OutputPresentationTransform::FlipY`]. GLES2 has no `GL_PACK_ROW_LENGTH`, so each row is read
/// separately and padding in `dst` is left unchanged. The caller keeps this off the KMS submission
/// path.
pub(crate) fn read_flip_y_framebuffer(
    framebuffer: gl::GLuint,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    dst_stride: usize,
    dst: &mut [u8],
) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    let row_bytes = (width as usize).saturating_mul(4);
    if dst_stride < row_bytes {
        return false;
    }
    let Some(needed) = dst_stride.checked_mul(height as usize) else {
        return false;
    };
    if dst.len() < needed {
        return false;
    }
    unsafe {
        let mut previous_framebuffer = 0;
        let mut previous_alignment = 4;
        gl::GetIntegerv(gl::FRAMEBUFFER_BINDING as _, &mut previous_framebuffer);
        gl::GetIntegerv(gl::PACK_ALIGNMENT as _, &mut previous_alignment);
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer);
        gl::PixelStorei(gl::PACK_ALIGNMENT as _, 1);
        gl::Finish();
        while gl::GetError() != gl::NO_ERROR as _ {}
        let mut ok = true;
        for row in 0..height {
            let dst_start = (row as usize).saturating_mul(dst_stride);
            gl::ReadPixels(
                x,
                y.saturating_add(row),
                width,
                1,
                gl::BGRA_EXT as _,
                gl::UNSIGNED_BYTE as _,
                dst[dst_start..].as_mut_ptr().cast(),
            );
            if gl::GetError() != gl::NO_ERROR as _ {
                ok = false;
                break;
            }
        }
        gl::PixelStorei(gl::PACK_ALIGNMENT as _, previous_alignment);
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, previous_framebuffer as gl::GLuint);
        ok
    }
}
