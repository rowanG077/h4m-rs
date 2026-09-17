#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod audio;
#[cfg(feature = "std")]
pub use audio::AudioDecoder;
pub use audio::{
    AudioBufferRequirements, AudioInfo, AudioLimits, AudioPacketDecoder, AudioPacketMode,
    SliceAudioDecoder,
};
mod container;
mod entropy;
mod error;
mod storage;
mod syntax;
mod video;

#[cfg(feature = "std")]
pub use container::VideoDecoder;
pub use container::{Header, SliceVideoDecoder};
pub use error::{BufferKind, Error};
pub use syntax::BlockState;
pub use video::{VideoBuffers, VideoPacketDecoder};

/// Video decoder backed by caller-owned mutable slices.
pub type BorrowedVideoPacketDecoder<'a> = VideoPacketDecoder<&'a mut [u8], &'a mut [BlockState]>;

/// Video decoder backed by vectors allocated once at construction.
#[cfg(feature = "alloc")]
pub type OwnedVideoPacketDecoder =
    VideoPacketDecoder<alloc::vec::Vec<u8>, alloc::vec::Vec<BlockState>>;

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
pub struct VideoLimits {
    /// Maximum luma pixels per frame. Default: 4096 × 4096.
    pub max_pixels: usize,
    /// Maximum compressed picture payload bytes, excluding the four-byte display
    /// index and eight-byte container packet header. Default: 64 MiB.
    /// Uses the same units for raw-packet and container decoders.
    pub max_frame_bytes: usize,
}

impl Default for VideoLimits {
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
pub struct VideoBufferRequirements {
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
    pub fn buffer_requirements(self) -> VideoBufferRequirements {
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        let chroma_width = width / usize::from(self.sampling.horizontal_factor());
        let chroma_height = height / usize::from(self.sampling.vertical_factor());
        VideoBufferRequirements {
            frame_bytes: width * height + 2 * chroma_width * chroma_height,
            block_states: (width / 4 + 2) * (height / 4 + 2)
                + 2 * (chroma_width / 4 + 2) * (chroma_height / 4 + 2),
        }
    }

    /// Bytes required for packed RGB24 output. Independent of resource limits.
    pub fn rgb_buffer_size(self) -> usize {
        usize::from(self.width) * usize::from(self.height) * 3
    }

    /// Check the pixel requirement against a resource policy before allocating.
    /// Packet byte limits are checked by the decoder when packets are supplied.
    pub fn validate_limits(self, limits: VideoLimits) -> Result<(), Error> {
        if usize::from(self.width) * usize::from(self.height) > limits.max_pixels {
            return Err(Error::Limit("pixel count"));
        }
        Ok(())
    }
}

/// A validated, tightly packed, borrowed eight-bit image plane.
/// Obtain planes from [`Frame::y`], [`Frame::u`] and [`Frame::v`].
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
}

impl<'a> Plane<'a> {
    /// Samples in row-major order; length is exactly width times height.
    pub const fn data(self) -> &'a [u8] {
        self.data
    }

    /// Samples per row (also the stride in bytes).
    pub const fn width(self) -> usize {
        self.width
    }

    /// Number of rows.
    pub const fn height(self) -> usize {
        self.height
    }
}

/// A validated, immutable decoded frame, borrowing reusable storage.
///
/// Decoders produce frames automatically. Use [`Self::from_planes`] to wrap
/// external tightly packed planes; sizes are checked before a frame is created.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    display_index: u32,
    kind: FrameType,
    info: VideoInfo,
    y: Plane<'a>,
    u: Plane<'a>,
    v: Plane<'a>,
}

impl<'a> Frame<'a> {
    /// Validate three tightly packed Y/U/V planes against a video layout.
    /// Plane lengths must exactly match the layout; padded strides are unsupported.
    pub fn from_planes(
        info: VideoInfo,
        kind: FrameType,
        display_index: u32,
        planes: [&'a [u8]; 3],
    ) -> Result<Self, Error> {
        let width = usize::from(info.width);
        let height = usize::from(info.height);
        let cw = width / usize::from(info.sampling.horizontal_factor());
        let ch = height / usize::from(info.sampling.vertical_factor());
        if planes[0].len() != width * height
            || planes[1].len() != cw * ch
            || planes[2].len() != cw * ch
        {
            return Err(Error::Invalid("plane lengths disagree with video layout"));
        }
        Ok(Self {
            info,
            kind,
            display_index,
            y: Plane {
                data: planes[0],
                width,
                height,
            },
            u: Plane {
                data: planes[1],
                width: cw,
                height: ch,
            },
            v: Plane {
                data: planes[2],
                width: cw,
                height: ch,
            },
        })
    }

    pub(crate) fn new(
        info: VideoInfo,
        kind: FrameType,
        display_index: u32,
        data: &'a [u8],
    ) -> Result<Self, Error> {
        let width = usize::from(info.width);
        let height = usize::from(info.height);
        let cw = width / usize::from(info.sampling.horizontal_factor());
        let ch = height / usize::from(info.sampling.vertical_factor());
        let (y, rest) = data
            .split_at_checked(width * height)
            .ok_or(Error::Truncated)?;
        let (u, v) = rest.split_at_checked(cw * ch).ok_or(Error::Truncated)?;
        Self::from_planes(info, kind, display_index, [y, u, v])
    }

    /// Zero-based presentation index, including the container's GOP offset.
    pub const fn display_index(self) -> u32 {
        self.display_index
    }

    /// Picture coding type.
    pub const fn kind(self) -> FrameType {
        self.kind
    }

    /// Validated video layout.
    pub const fn info(self) -> VideoInfo {
        self.info
    }

    /// Luma plane.
    pub const fn y(self) -> Plane<'a> {
        self.y
    }

    /// Blue-difference chroma plane.
    pub const fn u(self) -> Plane<'a> {
        self.u
    }

    /// Red-difference chroma plane.
    pub const fn v(self) -> Plane<'a> {
        self.v
    }

    /// Y, U, V planes in order.
    pub const fn planes(self) -> [Plane<'a>; 3] {
        [self.y, self.u, self.v]
    }

    /// Bytes needed for packed RGB24 output.
    pub fn rgb_buffer_size(self) -> usize {
        self.info.rgb_buffer_size()
    }

    /// Convert to packed RGB24 in caller-provided storage without allocating.
    /// Extra output bytes are unchanged. Insufficient storage leaves output untouched.
    /// Uses full-range YUV, nearest-neighbor chroma sampling, and truncation.
    pub fn to_rgb_into(&self, output: &mut [u8]) -> Result<(), Error> {
        let required = self.rgb_buffer_size();
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

    /// Convert to RGB24, growing/reusing a vector. Allocation failures return an error.
    #[cfg(feature = "alloc")]
    pub fn to_rgb(&self, output: &mut alloc::vec::Vec<u8>) -> Result<(), Error> {
        let required = self.rgb_buffer_size();
        output.try_reserve(required.saturating_sub(output.len()))?;
        output.resize(required, 0);
        self.to_rgb_into(output)
    }

    /// Write a binary PPM image, reusing `scratch` for RGB conversion.
    #[cfg(feature = "std")]
    pub fn write_ppm<W: std::io::Write>(
        &self,
        mut writer: W,
        scratch: &mut alloc::vec::Vec<u8>,
    ) -> Result<(), Error> {
        self.to_rgb(scratch)?;
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
