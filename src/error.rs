use std::{fmt, io};

/// A container, bitstream, resource-limit, or input/output error.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The input could not be read or output could not be written.
    Io(io::Error),
    /// The input ended inside a header or compressed stream.
    Truncated,
    /// The input violates the format or uses an unsupported coding mode.
    Invalid(&'static str),
    /// A configured resource limit was exceeded.
    Limit(&'static str),
    /// Decoding previously failed; construct a new decoder to resume.
    Failed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Truncated => f.write_str("truncated H4M data"),
            Self::Invalid(s) => write!(f, "invalid H4M data: {s}"),
            Self::Limit(s) => write!(f, "H4M resource limit exceeded: {s}"),
            Self::Failed => f.write_str("decoder cannot continue after an error"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Io(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            Self::Truncated
        } else {
            Self::Io(e)
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

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
