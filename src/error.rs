use core::fmt;
#[cfg(feature = "std")]
use std::io;

/// A container, bitstream, resource-limit, or input/output error.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The input could not be read or output could not be written.
    #[cfg(feature = "std")]
    Io(io::Error),
    /// Storage could not be allocated or its capacity cannot be represented.
    #[cfg(feature = "alloc")]
    Allocation(alloc::collections::TryReserveError),
    /// The input ended inside a header or compressed stream.
    Truncated,
    /// The input violates the format or uses an unsupported coding mode.
    Invalid(&'static str),
    /// A resource or platform representability limit was exceeded.
    Limit(&'static str),
    /// Decoding previously failed; construct a new decoder to resume.
    Failed,
    /// Caller-provided output or workspace is too small.
    BufferTooSmall {
        /// Which buffer needs more space.
        buffer: BufferKind,
        /// Required byte or element count.
        required: usize,
        /// Supplied byte or element count.
        provided: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "std")]
            Self::Io(e) => e.fmt(f),
            #[cfg(feature = "alloc")]
            Self::Allocation(e) => e.fmt(f),
            Self::Truncated => f.write_str("truncated H4M data"),
            Self::Invalid(s) => write!(f, "invalid H4M data: {s}"),
            Self::Limit(s) => write!(f, "H4M resource limit exceeded: {s}"),
            Self::BufferTooSmall {
                buffer,
                required,
                provided,
            } => write!(
                f,
                "{buffer:?} buffer needs {required} elements, received {provided}"
            ),
            Self::Failed => f.write_str("decoder cannot continue after an error"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        #[cfg(feature = "std")]
        if let Self::Io(error) = self {
            return Some(error);
        }
        #[cfg(feature = "alloc")]
        if let Self::Allocation(error) = self {
            return Some(error);
        }
        None
    }
}

#[cfg(feature = "std")]
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            Self::Truncated
        } else {
            #[cfg(feature = "std")]
            Self::Io(e)
        }
    }
}

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub(crate) fn be16(data: &[u8], offset: usize) -> Result<u16> {
    let v = data
        .get(offset..offset.checked_add(2).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u16::from_be_bytes([v[0], v[1]]))
}

pub(crate) fn be32(data: &[u8], offset: usize) -> Result<u32> {
    let v = data
        .get(offset..offset.checked_add(4).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
}

/// Purpose of a caller-owned buffer that was too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// One of the three byte-oriented Y/U/V frame buffers.
    Frame,
    /// Block-descriptor workspace, measured in `BlockState` elements.
    BlockStates,
    /// Packed RGB output bytes.
    Rgb,
    /// Interleaved PCM16 audio output, measured in `i16` elements.
    AudioPcm,
}

#[cfg(feature = "alloc")]
impl From<alloc::collections::TryReserveError> for Error {
    fn from(error: alloc::collections::TryReserveError) -> Self {
        Self::Allocation(error)
    }
}
