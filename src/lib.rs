//! Pure Rust decoding of HVQM4 1.3 and 1.5 (`.h4m`) video.
//!
//! [`Decoder`] reads the container from any [`std::io::Read`], skips audio, and
//! returns borrowed planar frames in **decoding order**. Use [`Frame::display_index`]
//! to put B frames in presentation order. [`VideoDecoder`] accepts demuxed packets.
//!
//! ```no_run
//! use std::{fs::File, io::BufReader};
//! let mut decoder = h4m::Decoder::new(BufReader::new(File::open("movie.h4m")?))?;
//! let mut rgb = Vec::new();
//! while let Some(frame) = decoder.next_frame()? {
//!     frame.to_rgb(&mut rgb);
//!     println!("frame {}: {} RGB bytes", frame.display_index, rgb.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]

mod container;
mod entropy;
mod error;
mod video;

pub use container::{Decoder, Header};
pub use error::Error;
pub use video::VideoDecoder;

/// HVQM4 bitstream version (affects chroma motion interpolation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// HVQM4 1.3.
    V13,
    /// HVQM4 1.5.
    V15,
}

/// Picture coding type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Independently coded picture.
    I,
    /// Picture predicted from the previous reference.
    P,
    /// Picture predicted from past and future references.
    B,
}

impl FrameType {
    pub(crate) fn parse(value: u16) -> Result<Self, Error> {
        match value {
            0x10 => Ok(Self::I),
            0x20 => Ok(Self::P),
            0x30 => Ok(Self::B),
            _ => Err(Error::Invalid("unknown frame type")),
        }
    }
}

/// Allocation limits applied before allocating pixel or packet buffers.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum luma pixels per frame. Default: 4096 × 4096.
    pub max_pixels: usize,
    /// Maximum compressed video packet size. Default: 64 MiB.
    pub max_frame_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pixels: 4096 * 4096,
            max_frame_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Video layout and codec version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInfo {
    /// Codec version.
    pub version: Version,
    /// Width in luma samples; must be a nonzero multiple of eight.
    pub width: u16,
    /// Height in luma samples; must be a nonzero multiple of eight.
    pub height: u16,
    /// Luma samples per chroma sample horizontally (1 or 2).
    pub horizontal_sampling: u8,
    /// Luma samples per chroma sample vertically (1 or 2).
    pub vertical_sampling: u8,
}

impl VideoInfo {
    pub(crate) fn validate(self, limits: Limits) -> Result<(), Error> {
        if self.width == 0
            || self.height == 0
            || !self.width.is_multiple_of(8)
            || !self.height.is_multiple_of(8)
        {
            return Err(Error::Invalid(
                "dimensions must be nonzero multiples of eight",
            ));
        }
        if !matches!(
            (self.horizontal_sampling, self.vertical_sampling),
            (1, 1) | (2, 1) | (2, 2)
        ) {
            return Err(Error::Invalid(
                "unsupported chroma sampling (expected 4:4:4, 4:2:2 or 4:2:0)",
            ));
        }
        let pixels = usize::from(self.width)
            .checked_mul(usize::from(self.height))
            .ok_or(Error::Limit("pixel count"))?;
        if pixels > limits.max_pixels || pixels > isize::MAX as usize / 9 {
            return Err(Error::Limit("pixel count"));
        }
        Ok(())
    }
}

/// A tightly packed, borrowed, eight-bit image plane.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    /// Samples in row-major order.
    pub data: &'a [u8],
    /// Samples per row (also the stride).
    pub width: usize,
    /// Number of rows.
    pub height: usize,
}

/// A decoded frame, borrowing reusable decoder storage.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// Zero-based presentation index, including the container's GOP offset.
    pub display_index: u32,
    /// Picture coding type.
    pub kind: FrameType,
    /// Video layout.
    pub info: VideoInfo,
    /// Luma plane.
    pub y: Plane<'a>,
    /// Blue-difference chroma plane.
    pub u: Plane<'a>,
    /// Red-difference chroma plane.
    pub v: Plane<'a>,
}

impl<'a> Frame<'a> {
    pub(crate) fn new(
        info: VideoInfo,
        kind: FrameType,
        display_index: u32,
        data: &'a [u8],
    ) -> Self {
        let width = usize::from(info.width);
        let height = usize::from(info.height);
        let cw = width / usize::from(info.horizontal_sampling);
        let ch = height / usize::from(info.vertical_sampling);
        let (y, rest) = data.split_at(width * height);
        let (u, v) = rest.split_at(cw * ch);
        Self {
            info,
            kind,
            display_index,
            y: Plane {
                data: y,
                width,
                height,
            },
            u: Plane {
                data: u,
                width: cw,
                height: ch,
            },
            v: Plane {
                data: v,
                width: cw,
                height: ch,
            },
        }
    }

    /// Convert to packed RGB24, reusing the allocation in `output`.
    ///
    /// Uses the reference decoder's full-range YUV conversion, nearest-neighbor
    /// chroma sampling, and truncation. No fused multiply-add is used.
    pub fn to_rgb(&self, output: &mut Vec<u8>) {
        output.resize(self.y.data.len() * 3, 0);
        let hs = usize::from(self.info.horizontal_sampling);
        let vs = usize::from(self.info.vertical_sampling);
        for row in 0..self.y.height {
            for col in 0..self.y.width {
                let pos = row * self.y.width + col;
                let uv = (row / vs) * self.u.width + col / hs;
                let y = f32::from(self.y.data[pos]);
                let u = f32::from(self.u.data[uv]) - 128.0;
                let v = f32::from(self.v.data[uv]) - 128.0;
                output[pos * 3] = (y + 1.402 * v) as u8;
                output[pos * 3 + 1] = (y - 0.34414 * u - 0.71414 * v) as u8;
                output[pos * 3 + 2] = (y + 1.772 * u) as u8;
            }
        }
    }

    /// Write one binary PPM image, reusing `scratch` for RGB conversion.
    pub fn write_ppm<W: std::io::Write>(
        &self,
        mut writer: W,
        scratch: &mut Vec<u8>,
    ) -> Result<(), Error> {
        self.to_rgb(scratch);
        write!(writer, "P6\n{} {}\n255\n", self.y.width, self.y.height)?;
        writer.write_all(scratch)?;
        Ok(())
    }

    /// Write tightly packed Y, U, V planes in that order.
    pub fn write_yuv<W: std::io::Write>(&self, mut writer: W) -> Result<(), Error> {
        writer.write_all(self.y.data)?;
        writer.write_all(self.u.data)?;
        writer.write_all(self.v.data)?;
        Ok(())
    }
}
