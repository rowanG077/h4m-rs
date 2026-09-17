//! Dependency-free HVQM4 1.3/1.5 decoding with no unsafe code.
//!
//! [`VideoDecoder`] decodes demuxed packets and [`SliceDecoder`] reads an H4M
//! byte slice. Both accept caller-provided buffers and work without `std` or
//! `alloc`. The default `std` feature adds the allocating, streaming `Decoder`
//! and file-output helpers. The independent `alloc` feature adds owned video
//! buffers and a reusable RGB vector helper.
//!
//! Frames borrow decoder storage and arrive in **decoding order**; use
//! [`Frame::display_index`] to reorder them for presentation. Audio is skipped.
//!
//! ```
//! use h4m::{BlockState, ChromaSampling, DecoderBuffers, Limits, Version,
//!           VideoDecoder, VideoInfo};
//! let info = VideoInfo::new(Version::V15, 16, 16, ChromaSampling::Yuv420)?;
//! let mut frames = [[0u8; 384]; 3];
//! let mut blocks = [BlockState::EMPTY; 68];
//! let [current, past, future] = &mut frames;
//! let mut decoder = VideoDecoder::with_buffers(info, DecoderBuffers {
//!     frames: [current.as_mut_slice(), past.as_mut_slice(), future.as_mut_slice()],
//!     blocks: blocks.as_mut_slice(),
//! }, Limits::default())?;
//! // decoder.decode(kind, display_index, packet)?;
//! # Ok::<(), h4m::Error>(())
//! ```
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod container;
mod entropy;
mod error;
mod syntax;
mod video;

#[cfg(feature = "std")]
pub use container::Decoder;
pub use container::{Header, SliceDecoder};
pub use error::{BufferKind, Error};
pub use syntax::BlockState;
pub use video::{DecoderBuffers, VideoDecoder};

/// Video decoder backed by caller-owned mutable slices.
pub type BorrowedVideoDecoder<'a> = VideoDecoder<&'a mut [u8], &'a mut [BlockState]>;

/// Video decoder backed by vectors allocated once at construction.
#[cfg(feature = "alloc")]
pub type OwnedVideoDecoder = VideoDecoder<alloc::vec::Vec<u8>, alloc::vec::Vec<BlockState>>;

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
#[repr(u16)]
pub enum FrameType {
    /// Independently coded picture.
    I = 0x10,
    /// Picture predicted from the past reference or current destination.
    P = 0x20,
    /// Picture predicted from past and future references.
    B = 0x30,
}

impl TryFrom<u16> for FrameType {
    type Error = Error;
    fn try_from(value: u16) -> Result<Self, Error> {
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

/// Supported chroma subsampling layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSampling {
    /// Full-resolution chroma in both dimensions.
    Yuv444,
    /// Half-resolution chroma horizontally.
    Yuv422,
    /// Half-resolution chroma in both dimensions.
    Yuv420,
}
impl ChromaSampling {
    /// Number of luma samples per chroma sample horizontally.
    pub const fn horizontal_factor(self) -> u8 {
        match self {
            Self::Yuv444 => 1,
            Self::Yuv422 | Self::Yuv420 => 2,
        }
    }
    /// Number of luma samples per chroma sample vertically.
    pub const fn vertical_factor(self) -> u8 {
        match self {
            Self::Yuv444 | Self::Yuv422 => 1,
            Self::Yuv420 => 2,
        }
    }
}
impl TryFrom<(u8, u8)> for ChromaSampling {
    type Error = Error;
    fn try_from(factors: (u8, u8)) -> Result<Self, Error> {
        match factors {
            (1, 1) => Ok(Self::Yuv444),
            (2, 1) => Ok(Self::Yuv422),
            (2, 2) => Ok(Self::Yuv420),
            _ => Err(Error::Invalid(
                "unsupported chroma sampling (expected 4:4:4, 4:2:2 or 4:2:0)",
            )),
        }
    }
}

/// Required reusable storage for one video decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRequirements {
    /// Bytes in **each** of the three frame buffers, including all Y/U/V planes.
    pub frame_bytes: usize,
    /// Elements in the block-descriptor buffer (not bytes).
    pub block_states: usize,
}

/// Validated, immutable video layout and codec version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInfo {
    version: Version,
    width: u16,
    height: u16,
    sampling: ChromaSampling,
}
impl VideoInfo {
    /// Validate dimensions and construct a video format without allocating.
    /// Width and height must be nonzero multiples of eight.
    pub fn new(
        version: Version,
        width: u16,
        height: u16,
        sampling: ChromaSampling,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || !width.is_multiple_of(8) || !height.is_multiple_of(8) {
            return Err(Error::Invalid(
                "dimensions must be nonzero multiples of eight",
            ));
        }
        let pixels = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or(Error::Limit("pixel count"))?;
        if pixels > isize::MAX as usize / 9 {
            return Err(Error::Limit("pixel count"));
        }
        Ok(Self {
            version,
            width,
            height,
            sampling,
        })
    }
    /// Codec version.
    pub const fn version(self) -> Version {
        self.version
    }
    /// Width in luma samples.
    pub const fn width(self) -> u16 {
        self.width
    }
    /// Height in luma samples.
    pub const fn height(self) -> u16 {
        self.height
    }
    /// Chroma subsampling layout.
    pub const fn sampling(self) -> ChromaSampling {
        self.sampling
    }
    /// Storage sizes for caller-owned buffers. This does not allocate.
    pub fn buffer_requirements(self) -> BufferRequirements {
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        let chroma_width = width / usize::from(self.sampling.horizontal_factor());
        let chroma_height = height / usize::from(self.sampling.vertical_factor());
        BufferRequirements {
            frame_bytes: width * height + 2 * chroma_width * chroma_height,
            block_states: (width / 4 + 2) * (height / 4 + 2)
                + 2 * (chroma_width / 4 + 2) * (chroma_height / 4 + 2),
        }
    }
    pub(crate) fn validate_limits(self, limits: Limits) -> Result<(), Error> {
        if usize::from(self.width) * usize::from(self.height) > limits.max_pixels {
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
        let cw = width / usize::from(info.sampling.horizontal_factor());
        let ch = height / usize::from(info.sampling.vertical_factor());
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

    /// Convert to packed RGB24 in caller-provided storage without allocating.
    /// Extra bytes in `output` are left unchanged.
    ///
    /// Uses the reference decoder's full-range YUV conversion, nearest-neighbor
    /// chroma sampling, and truncation. No fused multiply-add is used.
    pub fn to_rgb_into(&self, output: &mut [u8]) -> Result<(), Error> {
        let required = self.y.data.len() * 3;
        if output.len() < required {
            return Err(Error::BufferTooSmall {
                buffer: BufferKind::Rgb,
                required,
                provided: output.len(),
            });
        }
        let output = &mut output[..required];
        let hs = usize::from(self.info.sampling.horizontal_factor());
        let vs = usize::from(self.info.sampling.vertical_factor());
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
        Ok(())
    }

    /// Convert to packed RGB24, growing and reusing `output` as needed.
    #[cfg(feature = "alloc")]
    pub fn to_rgb(&self, output: &mut alloc::vec::Vec<u8>) {
        output.resize(self.y.data.len() * 3, 0);
        self.to_rgb_into(output)
            .expect("RGB buffer has the required length");
    }

    /// Write one binary PPM image, reusing `scratch` for RGB conversion.
    #[cfg(feature = "std")]
    pub fn write_ppm<W: std::io::Write>(
        &self,
        mut writer: W,
        scratch: &mut alloc::vec::Vec<u8>,
    ) -> Result<(), Error> {
        self.to_rgb(scratch);
        write!(writer, "P6\n{} {}\n255\n", self.y.width, self.y.height)?;
        writer.write_all(scratch)?;
        Ok(())
    }

    /// Write tightly packed Y, U, V planes in that order.
    #[cfg(feature = "std")]
    pub fn write_yuv<W: std::io::Write>(&self, mut writer: W) -> Result<(), Error> {
        writer.write_all(self.y.data)?;
        writer.write_all(self.u.data)?;
        writer.write_all(self.v.data)?;
        Ok(())
    }
}
