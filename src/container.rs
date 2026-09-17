use crate::{
    error::{be16, be32, Result},
    BlockState, Error, Frame, FrameType, Version, VideoBuffers, VideoInfo, VideoLimits,
    VideoPacketDecoder,
};
#[cfg(feature = "std")]
use std::io::Read;

/// Parsed 68-byte H4M file header.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    /// Video dimensions, sampling, and codec version.
    pub(crate) video: VideoInfo,
    /// Declared bytes following the file header.
    pub(crate) body_size: u32,
    /// Number of independently framed container blocks (GOPs).
    pub(crate) blocks: u32,
    /// Total number of video frames.
    pub(crate) video_frames: u32,
    /// Total number of audio packets.
    pub(crate) audio_packets: u32,
    /// Frame duration in microseconds.
    pub(crate) microseconds_per_frame: u32,
    /// Declared maximum video packet bytes, including its four-byte display index.
    pub(crate) max_video_packet_size: u32,
    /// Raw codec format byte (not a channel count).
    pub(crate) audio_format: u8,
    pub(crate) audio_bits: u8,
    pub(crate) audio_flags: u8,
    pub(crate) audio_tracks: u16,
    pub(crate) max_audio_packet_size: u32,
    /// Audio sample rate, if present.
    pub(crate) audio_sample_rate: u32,
}

impl Header {
    /// Encoded file header length in bytes.
    pub const SIZE: usize = 68;

    /// Validated video layout.
    pub const fn video(self) -> VideoInfo {
        self.video
    }

    /// Declared bytes after the header; video readers leave body padding unread.
    pub const fn body_size(self) -> u32 {
        self.body_size
    }

    /// Declared container block count.
    pub const fn blocks(self) -> u32 {
        self.blocks
    }

    /// Declared video frame count.
    pub const fn video_frames(self) -> u32 {
        self.video_frames
    }

    /// Declared audio packet count.
    pub const fn audio_packets(self) -> u32 {
        self.audio_packets
    }

    /// Video frame duration in microseconds.
    pub const fn microseconds_per_frame(self) -> u32 {
        self.microseconds_per_frame
    }

    /// Declared maximum video packet bytes, including its four-byte display index.
    pub const fn max_video_packet_size(self) -> u32 {
        self.max_video_packet_size
    }

    /// Raw audio codec format identifier; this is not a channel count.
    pub const fn audio_format(self) -> u8 {
        self.audio_format
    }

    /// Raw audio sample bit-depth field.
    pub const fn audio_bits(self) -> u8 {
        self.audio_bits
    }

    /// Raw audio codec flags.
    pub const fn audio_flags(self) -> u8 {
        self.audio_flags
    }

    /// Declared audio track count. Select tracks using one-based indices.
    pub const fn audio_tracks(self) -> u16 {
        self.audio_tracks
    }

    /// Declared maximum audio packet bytes, excluding the container packet header.
    pub const fn max_audio_packet_size(self) -> u32 {
        self.max_audio_packet_size
    }

    /// Declared audio sample rate; use audio_info to validate supported audio.
    pub const fn audio_sample_rate(self) -> u32 {
        self.audio_sample_rate
    }

    /// Validate the selected one-based audio track without reparsing the header.
    /// Video-only files and unsupported audio profiles return an error.
    pub fn audio_info(self, track: u16) -> Result<crate::AudioInfo> {
        crate::AudioInfo::from_header(self, track)
    }

    /// Check pixel and compressed picture payload bounds before allocating.
    pub fn validate_video_limits(self, limits: VideoLimits) -> Result<()> {
        self.video.validate_limits(limits)?;
        let bytes = usize::try_from(self.max_video_packet_size)
            .map_err(|_| Error::Limit("compressed frame size"))?;
        if bytes > isize::MAX as usize {
            return Err(Error::Limit("compressed frame size"));
        }
        if self.video_frames != 0 && bytes < 4 {
            return Err(Error::Invalid(
                "video packet maximum cannot hold display index",
            ));
        }
        if bytes.saturating_sub(4) > limits.max_frame_bytes {
            return Err(Error::Limit("compressed frame size"));
        }
        Ok(())
    }

    /// Parse a file header without allocating frame buffers.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
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
            video: VideoInfo::new(
                version,
                be16(data, 52)?,
                be16(data, 54)?,
                (data[56], data[57]).try_into()?,
            )?,
            body_size: be32(data, 20)?,
            blocks: be32(data, 24)?,
            video_frames: be32(data, 28)?,
            audio_packets: be32(data, 32)?,
            microseconds_per_frame: be32(data, 36)?,
            max_video_packet_size: be32(data, 40)?,
            audio_format: data[60],
            audio_bits: data[61],
            audio_flags: data[62],
            audio_tracks: u16::from(data[63]) + 1,
            max_audio_packet_size: be32(data, 48)?,
            audio_sample_rate: be32(data, 64)?,
        };
        if result.blocks == 0 {
            return Err(Error::Invalid("zero container blocks"));
        }
        Ok(result)
    }
}

/// Decode a complete in-memory H4M file without an I/O or allocation dependency.
///
/// Input packets are borrowed directly. Frame buffers and block workspace are
/// supplied to [`Self::with_buffers`]; no allocation occurs while decoding.
pub struct SliceVideoDecoder<'input, F, B> {
    inner: ContainerDecoder<SliceSource<'input>, F, B>,
}

impl<'input, F: AsRef<[u8]> + AsMut<[u8]>, B: AsMut<[BlockState]>> SliceVideoDecoder<'input, F, B> {
    /// Attach caller-owned buffers with default limits, without allocating.
    pub fn with_buffers(input: &'input [u8], buffers: VideoBuffers<F, B>) -> Result<Self> {
        Self::with_buffers_and_limits(input, buffers, VideoLimits::default())
    }

    /// Read the header and initialize caller-owned decoder buffers.
    pub fn with_buffers_and_limits(
        input: &'input [u8],
        buffers: VideoBuffers<F, B>,
        limits: VideoLimits,
    ) -> Result<Self> {
        let header = Header::parse(input)?;
        header.validate_video_limits(limits)?;
        let video = VideoPacketDecoder::with_buffers_and_limits(header.video, buffers, limits)?;
        Ok(Self {
            inner: ContainerDecoder::new(
                header,
                SliceSource {
                    remaining: &input[Header::SIZE..],
                },
                video,
                limits,
            )?,
        })
    }

    /// Validated video layout.
    pub fn metadata(&self) -> VideoInfo {
        self.inner.demux.header.video()
    }

    /// Parsed file metadata.
    pub fn header(&self) -> &Header {
        &self.inner.demux.header
    }

    /// Decode the next frame in decoding order, or `None` after the last block.
    /// An error permanently invalidates this instance.
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>> {
        self.inner.next_frame()
    }

    /// Recover the unconsumed input, including any trailing data, and storage.
    pub fn into_inner(self) -> (&'input [u8], VideoBuffers<F, B>) {
        (
            self.inner.demux.source.remaining,
            self.inner.video.into_buffers(),
        )
    }
}

/// Streaming H4M decoder with reusable, allocated buffers.
///
/// Available with `std`. Wrap files in `BufReader` for efficient small reads.
/// Construction allocates the packet, frame, and descriptor buffers once;
/// decoding does not grow them. Use [`SliceVideoDecoder`] for caller-owned buffers
/// and in-memory input without `std` or `alloc`.
#[cfg(feature = "std")]
pub struct VideoDecoder<R> {
    inner: ContainerDecoder<ReaderSource<R>, alloc::vec::Vec<u8>, alloc::vec::Vec<BlockState>>,
}

#[cfg(feature = "std")]
impl<R: Read> VideoDecoder<R> {
    /// Initialize a streaming decoder with default resource limits.
    pub fn new(reader: R) -> Result<Self> {
        Self::with_limits(reader, VideoLimits::default())
    }

    /// Read the header and allocate storage after validating resource limits.
    pub fn with_limits(mut reader: R, limits: VideoLimits) -> Result<Self> {
        let mut bytes = [0; Header::SIZE];
        reader.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes)?;
        header.validate_video_limits(limits)?;
        let video = VideoPacketDecoder::with_limits(header.video, limits)?;
        let source = ReaderSource {
            reader,
            scratch: crate::storage::filled_vec(
                (header.max_video_packet_size as usize).max(20),
                0,
            )?,
        };
        Ok(Self {
            inner: ContainerDecoder::new(header, source, video, limits)?,
        })
    }

    /// Validated video layout.
    pub fn metadata(&self) -> VideoInfo {
        self.inner.demux.header.video()
    }

    /// Parsed file metadata.
    pub fn header(&self) -> &Header {
        &self.inner.demux.header
    }

    /// Recover the reader, leaving trailing container data unread.
    pub fn into_inner(self) -> R {
        self.inner.demux.source.reader
    }

    /// Decode the next frame, or `None` after all declared blocks.
    /// After an error, subsequent calls return [`Error::Failed`].
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>> {
        self.inner.next_frame()
    }
}

#[derive(Clone, Copy)]
enum DecodeState {
    Ready,
    Finished,
    Failed,
}

struct ContainerDecoder<S, F, B> {
    demux: Demuxer<S>,
    video: VideoPacketDecoder<F, B>,
    state: DecodeState,
}

impl<S: Source, F: AsRef<[u8]> + AsMut<[u8]>, B: AsMut<[BlockState]>> ContainerDecoder<S, F, B> {
    fn new(
        header: Header,
        source: S,
        video: VideoPacketDecoder<F, B>,
        limits: VideoLimits,
    ) -> Result<Self> {
        header.validate_video_limits(limits)?;
        Ok(Self {
            demux: Demuxer {
                source,
                header,
                block: None,
                blocks_read: 0,
                read: PacketCounts::default(),
            },
            video,
            state: DecodeState::Ready,
        })
    }

    fn next_frame(&mut self) -> Result<Option<Frame<'_>>> {
        match self.state {
            DecodeState::Failed => return Err(Error::Failed),
            DecodeState::Finished => return Ok(None),
            DecodeState::Ready => {}
        }
        self.state = DecodeState::Failed;
        match self.demux.next_video()? {
            None => {
                self.state = DecodeState::Finished;
                Ok(None)
            }
            Some(packet) => {
                let frame = self
                    .video
                    .decode(packet.kind, packet.display_index, packet.data)?;
                self.state = DecodeState::Ready;
                Ok(Some(frame))
            }
        }
    }
}

/// The demuxer asks for short-lived input slices; slice input borrows directly,
/// while the std adapter fills its one preallocated scratch buffer.
trait Source {
    fn read_bytes(&mut self, size: usize) -> Result<&[u8]>;
    fn skip(&mut self, size: usize) -> Result<()>;
}

struct SliceSource<'a> {
    remaining: &'a [u8],
}

impl Source for SliceSource<'_> {
    fn read_bytes(&mut self, size: usize) -> Result<&[u8]> {
        let (bytes, rest) = self
            .remaining
            .split_at_checked(size)
            .ok_or(Error::Truncated)?;
        self.remaining = rest;
        Ok(bytes)
    }

    fn skip(&mut self, size: usize) -> Result<()> {
        self.read_bytes(size).map(|_| ())
    }
}

#[cfg(feature = "std")]
struct ReaderSource<R> {
    reader: R,
    scratch: alloc::vec::Vec<u8>,
}

#[cfg(feature = "std")]
impl<R: Read> Source for ReaderSource<R> {
    fn read_bytes(&mut self, size: usize) -> Result<&[u8]> {
        let target = self
            .scratch
            .get_mut(..size)
            .ok_or(Error::Invalid("packet exceeds declared maximum"))?;
        self.reader.read_exact(target)?;
        Ok(target)
    }

    fn skip(&mut self, mut size: usize) -> Result<()> {
        while size != 0 {
            let count = size.min(self.scratch.len());
            self.reader.read_exact(&mut self.scratch[..count])?;
            size -= count;
        }
        Ok(())
    }
}

#[derive(Default)]
struct PacketCounts {
    video: u32,
    audio: u32,
}

impl PacketCounts {
    fn is_empty(&self) -> bool {
        self.video == 0 && self.audio == 0
    }
}

struct BlockProgress {
    remaining_bytes: u32,
    remaining: PacketCounts,
    video_total: u32,
    first_display_index: u32,
}

impl BlockProgress {
    fn parse(bytes: &[u8], first_display_index: u32) -> Result<Self> {
        if be32(bytes, 16)? != 0x01000000 {
            return Err(Error::Invalid("container block marker"));
        }
        let video = be32(bytes, 8)?;
        Ok(Self {
            remaining_bytes: be32(bytes, 4)?,
            remaining: PacketCounts {
                video,
                audio: be32(bytes, 12)?,
            },
            video_total: video,
            first_display_index,
        })
    }

    fn consume_packet(&mut self, size: u32) -> Result<()> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(8)
            .ok_or(Error::Invalid("packet header exceeds block"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(size)
            .ok_or(Error::Invalid("packet exceeds block"))?;
        Ok(())
    }
}

enum PacketKind {
    Audio,
    Video(FrameType),
}

struct PacketHeader {
    kind: PacketKind,
    size: u32,
}

impl PacketHeader {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let kind = match be16(bytes, 0)? {
            0 => PacketKind::Audio,
            1 => PacketKind::Video(be16(bytes, 2)?.try_into()?),
            _ => return Err(Error::Invalid("unknown packet kind")),
        };
        Ok(Self {
            kind,
            size: be32(bytes, 4)?,
        })
    }
}

struct VideoPacket<'a> {
    kind: FrameType,
    display_index: u32,
    data: &'a [u8],
}

struct Demuxer<S> {
    source: S,
    header: Header,
    block: Option<BlockProgress>,
    blocks_read: u32,
    read: PacketCounts,
}

impl<S: Source> Demuxer<S> {
    fn next_video(&mut self) -> Result<Option<VideoPacket<'_>>> {
        loop {
            if self
                .block
                .as_ref()
                .is_none_or(|block| block.remaining.is_empty())
            {
                if self
                    .block
                    .as_ref()
                    .is_some_and(|block| block.remaining_bytes != 0)
                {
                    return Err(Error::Invalid("container block size mismatch"));
                }
                if self.blocks_read == self.header.blocks {
                    if self.read.video != self.header.video_frames
                        || self.read.audio != self.header.audio_packets
                    {
                        return Err(Error::Invalid("total frame count mismatch"));
                    }
                    return Ok(None);
                }
                let block = BlockProgress::parse(self.source.read_bytes(20)?, self.read.video)?;
                if block.remaining.video > self.header.video_frames - self.read.video
                    || block.remaining.audio > self.header.audio_packets - self.read.audio
                {
                    return Err(Error::Invalid("block frame count exceeds file total"));
                }
                self.block = Some(block);
                self.blocks_read += 1;
                continue;
            }
            let block = self
                .block
                .as_mut()
                .ok_or(Error::Invalid("missing container block"))?;
            if block.remaining_bytes < 8 {
                return Err(Error::Invalid("packet header exceeds block"));
            }
            let packet = PacketHeader::parse(self.source.read_bytes(8)?)?;
            block.consume_packet(packet.size)?;
            match packet.kind {
                PacketKind::Audio => {
                    block.remaining.audio = block
                        .remaining
                        .audio
                        .checked_sub(1)
                        .ok_or(Error::Invalid("too many audio packets"))?;
                    self.source.skip(packet.size as usize)?;
                    self.read.audio += 1;
                }
                PacketKind::Video(kind) => {
                    if block.remaining.video == 0 {
                        return Err(Error::Invalid("too many video packets"));
                    }
                    if packet.size > self.header.max_video_packet_size {
                        return Err(Error::Invalid("packet exceeds declared maximum"));
                    }
                    if block.remaining.video == block.video_total && kind != FrameType::I {
                        return Err(Error::Invalid("GOP must start with an I frame"));
                    }
                    let data = self.source.read_bytes(packet.size as usize)?;
                    let display = be32(data, 0)?;
                    if display >= block.video_total {
                        return Err(Error::Invalid("display index outside GOP"));
                    }
                    block.remaining.video -= 1;
                    self.read.video += 1;
                    return Ok(Some(VideoPacket {
                        kind,
                        display_index: block.first_display_index + display,
                        data: &data[4..],
                    }));
                }
            }
        }
    }
}
