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
        (
            rows[0][0] * x + rows[0][1] * y + rows[0][2],
            rows[1][0] * x + rows[1][1] * y + rows[1][2],
        )
    }

    pub fn to_gl_array(self) -> [f32; 9] {
        let rows = self.rows;
        [
            rows[0][0],
            rows[1][0],
            rows[2][0],
            rows[0][1],
            rows[1][1],
            rows[2][1],
            rows[0][2],
            rows[1][2],
            rows[2][2],
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputPresentationTransform {
    Normal,
    FlipY,
}

pub fn output_projection(
    width: f32,
    height: f32,
    transform: OutputPresentationTransform,
) -> Mat3 {
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

#[cfg(test)]
mod tests {
    use super::{
        OutputPresentationTransform, normalized_texcoord_flip_y, output_projection, project_box,
    };

    #[test]
    fn normal_projection_maps_top_left_box_to_clip_top_left() {
        let transform =
            project_box(0.0, 0.0, 100.0, 50.0, output_projection(200.0, 100.0, OutputPresentationTransform::Normal));

        assert_eq!(transform.transform_point(0.0, 0.0), (-1.0, 1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn flipped_projection_compensates_for_y_inverted_targets() {
        let transform =
            project_box(0.0, 0.0, 100.0, 50.0, output_projection(200.0, 100.0, OutputPresentationTransform::FlipY));

        assert_eq!(transform.transform_point(0.0, 0.0), (-1.0, -1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn normalized_texcoord_flip_y_maps_top_left_to_gl_upper_row() {
        let transform = normalized_texcoord_flip_y();

        assert_eq!(transform.transform_point(0.0, 0.0), (0.0, 1.0));
        assert_eq!(transform.transform_point(1.0, 1.0), (1.0, 0.0));
    }
}
