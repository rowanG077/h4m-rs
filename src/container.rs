use crate::{
    error::{be16, be32, Result},
    Error, Frame, FrameType, Limits, Version, VideoDecoder, VideoInfo,
};
use std::io::Read;

/// Parsed 68-byte H4M file header.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    /// Video dimensions, sampling, and codec version.
    pub video: VideoInfo,
    /// Declared bytes following the file header.
    pub body_size: u32,
    /// Number of independently framed container blocks (GOPs).
    pub blocks: u32,
    /// Total number of video frames.
    pub video_frames: u32,
    /// Total number of audio packets, skipped by this decoder.
    pub audio_frames: u32,
    /// Frame duration in microseconds.
    pub microseconds_per_frame: u32,
    /// Declared maximum compressed frame size.
    pub max_frame_size: u32,
    /// Audio channel count.
    pub audio_channels: u8,
    /// Audio sample rate, if present.
    pub audio_sample_rate: u32,
}

impl Header {
    /// Parse a file header without allocating frame buffers.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 68 {
            return Err(Error::Truncated);
        }
        let version = match &data[..16] {
            b"HVQM4 1.3\0\0\0\0\0\0\0" => Version::V13,
            b"HVQM4 1.5\0\0\0\0\0\0\0" => Version::V15,
            _ => return Err(Error::Invalid("file signature")),
        };
        if be32(data, 16)? != 68 {
            return Err(Error::Invalid("header size"));
        }
        if be32(data, 44)? != 0 || data[59] != 0 {
            return Err(Error::Invalid("reserved header fields"));
        }
        if !matches!(data[58], 0 | 0x12) {
            return Err(Error::Invalid("video mode"));
        }
        let result = Self {
            video: VideoInfo {
                version,
                width: be16(data, 52)?,
                height: be16(data, 54)?,
                horizontal_sampling: data[56],
                vertical_sampling: data[57],
            },
            body_size: be32(data, 20)?,
            blocks: be32(data, 24)?,
            video_frames: be32(data, 28)?,
            audio_frames: be32(data, 32)?,
            microseconds_per_frame: be32(data, 36)?,
            max_frame_size: be32(data, 40)?,
            audio_channels: data[60],
            audio_sample_rate: be32(data, 64)?,
        };
        if result.blocks == 0 {
            return Err(Error::Invalid("zero container blocks"));
        }
        Ok(result)
    }
}

/// Streaming H4M container reader and video decoder.
///
/// Wrap files in [`std::io::BufReader`] for efficient small header reads. Audio
/// packets are skipped without allocating them. Frames are returned in decoding
/// order, with presentation indices exposed on [`Frame`].
pub struct Decoder<R> {
    reader: R,
    header: Header,
    video: VideoDecoder,
    packet: Vec<u8>,
    limits: Limits,
    blocks_read: u32,
    block_bytes: u32,
    block_video: u32,
    block_audio: u32,
    block_video_total: u32,
    gop_start: u32,
    video_read: u32,
    audio_read: u32,
    failed: bool,
    finished: bool,
}

impl<R: Read> Decoder<R> {
    /// Read the file header and initialize the decoder with default limits.
    pub fn new(reader: R) -> Result<Self> {
        Self::with_limits(reader, Limits::default())
    }

    /// Read the file header using explicit allocation limits.
    pub fn with_limits(mut reader: R, limits: Limits) -> Result<Self> {
        let mut bytes = [0; 68];
        reader.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes)?;
        let video = VideoDecoder::with_limits(header.video, limits)?;
        Ok(Self {
            reader,
            header,
            video,
            limits,
            packet: Vec::new(),
            blocks_read: 0,
            block_bytes: 0,
            block_video: 0,
            block_audio: 0,
            block_video_total: 0,
            gop_start: 0,
            video_read: 0,
            audio_read: 0,
            failed: false,
            finished: false,
        })
    }

    /// File metadata.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Consume the decoder and recover its input reader.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Decode the next video frame. Returns `None` after all declared blocks.
    ///
    /// After an error the decoder is unusable and returns [`Error::Failed`].
    /// Container padding or trailing data beyond the declared blocks is left unread.
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>> {
        if self.failed {
            return Err(Error::Failed);
        }
        if self.finished {
            return Ok(None);
        }
        self.failed = true;
        loop {
            if self.block_video == 0 && self.block_audio == 0 {
                if self.block_bytes != 0 {
                    return Err(Error::Invalid("container block size mismatch"));
                }
                if self.blocks_read == self.header.blocks {
                    if self.video_read != self.header.video_frames
                        || self.audio_read != self.header.audio_frames
                    {
                        return Err(Error::Invalid("total frame count mismatch"));
                    }
                    self.finished = true;
                    self.failed = false;
                    return Ok(None);
                }
                let mut bytes = [0; 20];
                self.reader.read_exact(&mut bytes)?;
                if be32(&bytes, 16)? != 0x01000000 {
                    return Err(Error::Invalid("container block marker"));
                }
                self.block_bytes = be32(&bytes, 4)?;
                self.block_video = be32(&bytes, 8)?;
                self.block_audio = be32(&bytes, 12)?;
                self.block_video_total = self.block_video;
                self.gop_start = self.video_read;
                if self.block_video > self.header.video_frames - self.video_read
                    || self.block_audio > self.header.audio_frames - self.audio_read
                {
                    return Err(Error::Invalid("block frame count exceeds file total"));
                }
                self.blocks_read += 1;
                continue;
            }
            if self.block_bytes < 8 {
                return Err(Error::Invalid("packet header exceeds block"));
            }
            let mut bytes = [0; 8];
            self.reader.read_exact(&mut bytes)?;
            let size = be32(&bytes, 4)?;
            self.block_bytes -= 8;
            if size > self.block_bytes {
                return Err(Error::Invalid("packet exceeds block"));
            }
            self.block_bytes -= size;
            match be16(&bytes, 0)? {
                0 => {
                    if self.block_audio == 0 {
                        return Err(Error::Invalid("too many audio packets"));
                    }
                    let mut remaining = size as usize;
                    let mut scratch = [0; 8192];
                    while remaining > 0 {
                        let n = remaining.min(scratch.len());
                        self.reader.read_exact(&mut scratch[..n])?;
                        remaining -= n;
                    }
                    self.block_audio -= 1;
                    self.audio_read += 1;
                }
                1 => {
                    if self.block_video == 0 {
                        return Err(Error::Invalid("too many video packets"));
                    }
                    if size as usize > self.limits.max_frame_bytes {
                        return Err(Error::Limit("compressed frame size"));
                    }
                    let kind = FrameType::parse(be16(&bytes, 2)?)?;
                    if self.video_read == self.gop_start && kind != FrameType::I {
                        return Err(Error::Invalid("GOP must start with an I frame"));
                    }
                    self.packet.resize(size as usize, 0);
                    self.reader.read_exact(&mut self.packet)?;
                    let display = be32(&self.packet, 0)?;
                    if display >= self.block_video_total {
                        return Err(Error::Invalid("display index outside GOP"));
                    }
                    self.block_video -= 1;
                    self.video_read += 1;
                    let frame =
                        self.video
                            .decode(kind, self.gop_start + display, &self.packet[4..])?;
                    self.failed = false;
                    return Ok(Some(frame));
                }
                _ => return Err(Error::Invalid("unknown packet kind")),
            }
        }
    }
}
