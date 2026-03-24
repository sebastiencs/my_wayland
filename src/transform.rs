#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3 {
    rows: [[f32; 3]; 3],
}

impl Mat3 {
    pub const fn identity() -> Self {
        Self { rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] }
    }

    pub const fn translation(tx: f32, ty: f32) -> Self {
        Self { rows: [[1.0, 0.0, tx], [0.0, 1.0, ty], [0.0, 0.0, 1.0]] }
    }

    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self { rows: [[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]] }
    }

    pub fn mul(self, rhs: Self) -> Self {
        let mut rows = [[0.0; 3]; 3];
        let lhs = self.rows;
        let rhs = rhs.rows;

        for row in 0..3 {
            for col in 0..3 {
                rows[row][col] = lhs[row][0] * rhs[0][col]
                    + lhs[row][1] * rhs[1][col]
                    + lhs[row][2] * rhs[2][col];
            }
        }

        Self { rows }
    }

    pub fn transform_point(self, x: f32, y: f32) -> (f32, f32) {
        let rows = self.rows;
        (rows[0][0] * x + rows[0][1] * y + rows[0][2], rows[1][0] * x + rows[1][1] * y + rows[1][2])
    }

    pub fn inverse(self) -> Option<Self> {
        let m = self.rows;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < f32::EPSILON {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Self {
            rows: [
                [
                    (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
                    (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
                    (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
                ],
                [
                    (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
                    (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
                    (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
                ],
                [
                    (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
                    (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
                    (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
                ],
            ],
        })
    }

    pub fn to_gl_array(self) -> [f32; 9] {
        let rows = self.rows;
        [
            rows[0][0], rows[1][0], rows[2][0], rows[0][1], rows[1][1], rows[2][1], rows[0][2],
            rows[1][2], rows[2][2],
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferTransform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flip,
    FlipRotate90,
    FlipRotate180,
    FlipRotate270,
}

impl Default for BufferTransform {
    fn default() -> Self {
        Self::Normal
    }
}

impl BufferTransform {
    pub fn from_wayland(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Normal,
            1 => Self::Rotate90,
            2 => Self::Rotate180,
            3 => Self::Rotate270,
            4 => Self::Flip,
            5 => Self::FlipRotate90,
            6 => Self::FlipRotate180,
            7 => Self::FlipRotate270,
            _ => return None,
        })
    }

    pub fn maybe_swap<T>(self, (width, height): (T, T)) -> (T, T) {
        match self {
            Self::Normal | Self::Rotate180 | Self::Flip | Self::FlipRotate180 => (width, height),
            Self::Rotate90 | Self::Rotate270 | Self::FlipRotate90 | Self::FlipRotate270 => {
                (height, width)
            }
        }
    }

    pub fn normalized_inverse_matrix(self) -> Mat3 {
        match self {
            Self::Normal => Mat3::identity(),
            Self::Rotate90 => Mat3 { rows: [[0.0, 1.0, 0.0], [-1.0, 0.0, 1.0], [0.0, 0.0, 1.0]] },
            Self::Rotate180 => Mat3 { rows: [[-1.0, 0.0, 1.0], [0.0, -1.0, 1.0], [0.0, 0.0, 1.0]] },
            Self::Rotate270 => Mat3 { rows: [[0.0, -1.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] },
            Self::Flip => Mat3 { rows: [[-1.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] },
            Self::FlipRotate90 => {
                Mat3 { rows: [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] }
            }
            Self::FlipRotate180 => {
                Mat3 { rows: [[1.0, 0.0, 0.0], [0.0, -1.0, 1.0], [0.0, 0.0, 1.0]] }
            }
            Self::FlipRotate270 => {
                Mat3 { rows: [[0.0, -1.0, 1.0], [-1.0, 0.0, 1.0], [0.0, 0.0, 1.0]] }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SizeI {
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeF {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceViewport {
    pub src: Option<RectF>,
    pub dst: Option<SizeI>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputPresentationTransform {
    Normal,
    FlipY,
}

pub fn output_projection(width: f32, height: f32, transform: OutputPresentationTransform) -> Mat3 {
    assert!(width > 0.0, "output projection width must be positive");
    assert!(height > 0.0, "output projection height must be positive");

    match transform {
        OutputPresentationTransform::Normal => Mat3 {
            rows: [[2.0 / width, 0.0, -1.0], [0.0, -2.0 / height, 1.0], [0.0, 0.0, 1.0]],
        },
        OutputPresentationTransform::FlipY => Mat3 {
            rows: [[2.0 / width, 0.0, -1.0], [0.0, 2.0 / height, -1.0], [0.0, 0.0, 1.0]],
        },
    }
}

pub fn box_transform(x: f32, y: f32, width: f32, height: f32) -> Mat3 {
    Mat3::translation(x, y).mul(Mat3::scale(width, height))
}

pub fn project_box(x: f32, y: f32, width: f32, height: f32, projection: Mat3) -> Mat3 {
    projection.mul(box_transform(x, y, width, height))
}

pub fn normalized_texcoord_flip_y() -> Mat3 {
    Mat3::translation(0.0, 1.0).mul(Mat3::scale(1.0, -1.0))
}

pub fn transformed_buffer_size(buffer_size: SizeI, transform: BufferTransform) -> SizeI {
    let (width, height) = transform.maybe_swap((buffer_size.width, buffer_size.height));
    SizeI { width, height }
}

pub fn pre_viewport_logical_size(
    buffer_size: SizeI,
    transform: BufferTransform,
    buffer_scale: i32,
) -> SizeF {
    let transformed = transformed_buffer_size(buffer_size, transform);
    SizeF {
        width: transformed.width as f32 / buffer_scale as f32,
        height: transformed.height as f32 / buffer_scale as f32,
    }
}

pub fn derive_logical_size(
    buffer_size: SizeI,
    transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
) -> SizeI {
    if let Some(dst) = viewport.dst {
        return dst;
    }
    if let Some(src) = viewport.src {
        return SizeI {
            width: src.width.round() as i32,
            height: src.height.round() as i32,
        };
    }
    let transformed = transformed_buffer_size(buffer_size, transform);
    SizeI {
        width: (transformed.width + buffer_scale - 1) / buffer_scale,
        height: (transformed.height + buffer_scale - 1) / buffer_scale,
    }
}

pub fn viewport_src_transform(base_logical_size: SizeF, viewport: SurfaceViewport) -> Mat3 {
    let Some(src) = viewport.src else {
        return Mat3::identity();
    };
    Mat3::translation(src.x / base_logical_size.width, src.y / base_logical_size.height).mul(
        Mat3::scale(src.width / base_logical_size.width, src.height / base_logical_size.height),
    )
}

pub fn surface_sample_transform(
    buffer_size: SizeI,
    transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
) -> Mat3 {
    let base = pre_viewport_logical_size(buffer_size, transform, buffer_scale);
    transform
        .normalized_inverse_matrix()
        .mul(viewport_src_transform(base, viewport))
}

pub fn rect_contains(container: RectF, rect: RectF) -> bool {
    rect.x >= container.x
        && rect.y >= container.y
        && rect.x + rect.width <= container.x + container.width
        && rect.y + rect.height <= container.y + container.height
}

pub fn transform_rect_bounds(rect: RectF, matrix: Mat3) -> RectF {
    let corners = [
        matrix.transform_point(rect.x, rect.y),
        matrix.transform_point(rect.x + rect.width, rect.y),
        matrix.transform_point(rect.x, rect.y + rect.height),
        matrix.transform_point(rect.x + rect.width, rect.y + rect.height),
    ];
    let min_x = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|p| p.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = corners
        .iter()
        .map(|p| p.1)
        .fold(f32::NEG_INFINITY, f32::max);
    RectF {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

pub fn surface_rect_to_buffer_rect(
    rect: RectF,
    logical_size: SizeI,
    buffer_size: SizeI,
    transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
) -> RectF {
    let normalized = RectF {
        x: rect.x / logical_size.width as f32,
        y: rect.y / logical_size.height as f32,
        width: rect.width / logical_size.width as f32,
        height: rect.height / logical_size.height as f32,
    };
    let sample = surface_sample_transform(buffer_size, transform, buffer_scale, viewport);
    let mapped = transform_rect_bounds(normalized, sample);
    RectF {
        x: mapped.x * buffer_size.width as f32,
        y: mapped.y * buffer_size.height as f32,
        width: mapped.width * buffer_size.width as f32,
        height: mapped.height * buffer_size.height as f32,
    }
}

pub fn buffer_rect_to_surface_rect(
    rect: RectF,
    logical_size: SizeI,
    buffer_size: SizeI,
    transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
) -> Option<RectF> {
    let normalized = RectF {
        x: rect.x / buffer_size.width as f32,
        y: rect.y / buffer_size.height as f32,
        width: rect.width / buffer_size.width as f32,
        height: rect.height / buffer_size.height as f32,
    };
    let sample = surface_sample_transform(buffer_size, transform, buffer_scale, viewport);
    let inverse = sample.inverse()?;
    let mapped = transform_rect_bounds(normalized, inverse);
    Some(RectF {
        x: mapped.x * logical_size.width as f32,
        y: mapped.y * logical_size.height as f32,
        width: mapped.width * logical_size.width as f32,
        height: mapped.height * logical_size.height as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        buffer_rect_to_surface_rect, derive_logical_size, normalized_texcoord_flip_y,
        output_projection, pre_viewport_logical_size, project_box, rect_contains,
        surface_rect_to_buffer_rect, surface_sample_transform, transform_rect_bounds,
        BufferTransform, OutputPresentationTransform, RectF, SizeI, SurfaceViewport,
    };

    #[test]
    fn normal_projection_maps_top_left_box_to_clip_top_left() {
        let transform = project_box(
            0.0,
            0.0,
            100.0,
            50.0,
            output_projection(200.0, 100.0, OutputPresentationTransform::Normal),
        );

        assert_eq!(transform.transform_point(0.0, 0.0), (-1.0, 1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn flipped_projection_compensates_for_y_inverted_targets() {
        let transform = project_box(
            0.0,
            0.0,
            100.0,
            50.0,
            output_projection(200.0, 100.0, OutputPresentationTransform::FlipY),
        );

        assert_eq!(transform.transform_point(0.0, 0.0), (-1.0, -1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn normalized_texcoord_flip_y_maps_top_left_to_gl_upper_row() {
        let transform = normalized_texcoord_flip_y();

        assert_eq!(transform.transform_point(0.0, 0.0), (0.0, 1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (1.0, 0.0));
    }

    #[test]
    fn rotated_buffer_size_swaps_axes() {
        let base = pre_viewport_logical_size(
            SizeI { width: 640, height: 480 },
            BufferTransform::Rotate90,
            1,
        );
        assert_eq!(base.width, 480.0);
        assert_eq!(base.height, 640.0);
    }

    #[test]
    fn logical_size_uses_viewport_destination_when_present() {
        let size = derive_logical_size(
            SizeI { width: 640, height: 480 },
            BufferTransform::Normal,
            1,
            SurfaceViewport {
                src: Some(RectF { x: 10.0, y: 20.0, width: 30.0, height: 40.0 }),
                dst: Some(SizeI { width: 120, height: 80 }),
            },
        );
        assert_eq!(size, SizeI { width: 120, height: 80 });
    }

    #[test]
    fn rotate90_sample_transform_maps_surface_top_left_to_raw_bottom_left() {
        let transform = surface_sample_transform(
            SizeI { width: 640, height: 480 },
            BufferTransform::Rotate90,
            1,
            SurfaceViewport::default(),
        );
        assert_eq!(transform.transform_point(0.0, 0.0), (0.0, 1.0));
        assert_eq!(transform.transform_point(1.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn viewport_source_transform_crops_in_previewport_space() {
        let transform = surface_sample_transform(
            SizeI { width: 640, height: 480 },
            BufferTransform::Normal,
            2,
            SurfaceViewport {
                src: Some(RectF { x: 10.0, y: 20.0, width: 30.0, height: 40.0 }),
                dst: Some(SizeI { width: 100, height: 100 }),
            },
        );
        let p = transform.transform_point(0.0, 0.0);
        assert_eq!(p, (10.0 / 320.0, 20.0 / 240.0));
    }

    #[test]
    fn rect_contains_checks_buffer_bounds() {
        assert!(rect_contains(
            RectF { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
            RectF { x: 2.0, y: 3.0, width: 4.0, height: 5.0 }
        ));
        assert!(!rect_contains(
            RectF { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
            RectF { x: 8.0, y: 8.0, width: 4.0, height: 4.0 }
        ));
    }

    #[test]
    fn transform_rect_bounds_uses_bounding_box() {
        let rect = RectF { x: 0.0, y: 0.0, width: 0.25, height: 0.5 };
        let mapped =
            transform_rect_bounds(rect, BufferTransform::Rotate90.normalized_inverse_matrix());
        assert_eq!(mapped, RectF { x: 0.0, y: 0.75, width: 0.5, height: 0.25 });
    }

    #[test]
    fn damage_rect_roundtrip_between_surface_and_buffer_spaces() {
        let logical = SizeI { width: 320, height: 240 };
        let buffer = SizeI { width: 640, height: 480 };
        let surface_rect = RectF { x: 16.0, y: 20.0, width: 40.0, height: 30.0 };
        let buffer_rect = surface_rect_to_buffer_rect(
            surface_rect,
            logical,
            buffer,
            BufferTransform::Normal,
            2,
            SurfaceViewport::default(),
        );
        let roundtrip = buffer_rect_to_surface_rect(
            buffer_rect,
            logical,
            buffer,
            BufferTransform::Normal,
            2,
            SurfaceViewport::default(),
        )
        .unwrap();
        assert_eq!(roundtrip, surface_rect);
    }
}
