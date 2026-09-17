//! Checked motion sampling, including sequential copies within a P picture.

use super::plane::Layout;
use crate::{
    error::{Error, Result},
    syntax::MotionVector,
    Version,
};

#[derive(Clone, Copy)]
enum Interpolation {
    Integer,
    Horizontal,
    Vertical,
    Bilinear,
}

impl Interpolation {
    fn from_parity(x: i32, y: i32) -> Self {
        match (x & 1 != 0, y & 1 != 0) {
            (false, false) => Self::Integer,
            (true, false) => Self::Horizontal,
            (false, true) => Self::Vertical,
            (true, true) => Self::Bilinear,
        }
    }

    fn offsets(self) -> (usize, usize) {
        match self {
            Self::Integer => (0, 0),
            Self::Horizontal => (1, 0),
            Self::Vertical => (0, 1),
            Self::Bilinear => (1, 1),
        }
    }
}

/// A validated 4×4 source region. Construction performs bounds checks once.
pub(super) struct MotionSample {
    start: usize,
    stride: usize,
    interpolation: Interpolation,
}

impl MotionSample {
    pub fn new(
        layout: Layout,
        reference: MotionVector,
        sub: usize,
        version: Version,
    ) -> Result<Self> {
        let plane_x = reference.x >> layout.horizontal_shift;
        let plane_y = reference.y >> layout.vertical_shift;
        let interpolation = match version {
            Version::V13 => Interpolation::from_parity(reference.x, reference.y),
            Version::V15 => Interpolation::from_parity(plane_x, plane_y),
        };
        let (block_x, block_y) = layout.macroblock(0, 0, sub);
        let x = (plane_x >> 1) + (block_x * 4) as i32;
        let y = (plane_y >> 1) + (block_y * 4) as i32;
        let (horizontal, vertical) = interpolation.offsets();
        if x < 0
            || y < 0
            || x as usize + 4 + horizontal > layout.width
            || y as usize + 4 + vertical > layout.height
        {
            return Err(Error::Invalid("motion vector outside reference plane"));
        }
        Ok(Self {
            start: layout.offset + y as usize * layout.width + x as usize,
            stride: layout.width,
            interpolation,
        })
    }

    fn pixel(&self, source: &[u8], offset: usize) -> u8 {
        let (horizontal, vertical) = self.interpolation.offsets();
        let sum = u16::from(source[offset])
            + u16::from(source[offset + horizontal])
            + u16::from(source[offset + vertical * self.stride])
            + u16::from(source[offset + vertical * self.stride + horizontal]);
        ((sum + 2) >> 2) as u8
    }

    pub fn read(&self, source: &[u8]) -> [u8; 16] {
        let mut result = [0; 16];
        for (row, pixels) in result.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let start = self.start + row * self.stride;
            if matches!(self.interpolation, Interpolation::Integer) {
                pixels.copy_from_slice(&source[start..start + 4]);
            } else {
                for (column, pixel) in pixels.iter_mut().enumerate() {
                    *pixel = self.pixel(source, start + column);
                }
            }
        }
        result
    }

    pub fn copy_within(&self, destination: &mut [u8], target: usize) {
        // The C reference writes each pixel immediately. Snapshotting the block
        // would change overlapping current-picture references, even within a row.
        for row in 0..4 {
            for column in 0..4 {
                destination[target + row * self.stride + column] =
                    self.pixel(destination, self.start + row * self.stride + column);
            }
        }
    }
}
