// Format and IMA arithmetic checked against vgmstream r2117.
// Copyright/permission notice: LICENSE.vgmstream.
//! HVQM4 IMA-ADPCM audio (the stereo mode 3/2 profile).

use crate::{
    error::{be16, be32, Result},
    BufferKind, Error, Header,
};
#[cfg(feature = "std")]
use alloc::vec::Vec;
#[cfg(feature = "std")]
use std::io::Read;

/// Validated, immutable audio metadata for an explicitly selected one-based track.
///
/// Parse the header with [`Self::parse`], then call [`Self::buffer_requirements`]
/// before choosing PCM storage. Neither operation allocates or decodes a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioInfo {
    sample_rate: u32,
    tracks: u16,
    track: u16,
    packets: u32,
    requirements: AudioBufferRequirements,
}

impl AudioInfo {
    /// Parse the 68-byte H4M header and validate its audio profile and track.
    ///
    /// `track` is one-based. Bytes after the header are ignored. Packet contents
    /// and container totals are checked later during decoding. Buffer sizes are
    /// checked for representability here, so [`Self::buffer_requirements`] cannot
    /// fail and needs no resource policy.
    pub fn parse(bytes: &[u8], track: u16) -> Result<Self> {
        Header::parse(bytes)?.audio_info(track)
    }

    pub(crate) fn from_header(header: Header, track: u16) -> Result<Self> {
        if (header.audio_format, header.audio_bits, header.audio_flags) != (2, 16, 0)
            || header.audio_packets == 0
            || !(8000..=192000).contains(&header.audio_sample_rate)
        {
            return Err(Error::Invalid("unsupported IMA audio profile"));
        }
        let tracks = header.audio_tracks;
        if track == 0 || track > tracks {
            return Err(Error::Invalid("audio track is outside one-based range"));
        }
        let max_packet_bytes = usize::try_from(header.max_audio_packet_size)
            .map_err(|_| Error::Limit("audio packet bytes"))?;
        // At least one mode 3 sample: count + (two predictors + padding) per track.
        if max_packet_bytes < 4 + 7 * usize::from(tracks) {
            return Err(Error::Invalid(
                "audio packet maximum cannot hold initialization",
            ));
        }
        if max_packet_bytes > isize::MAX as usize {
            return Err(Error::Limit("audio packet bytes"));
        }
        // Mode 2 has one compressed byte per stereo frame in each track.
        // Mode 3 uses more bytes for predictors, so this bounds its output too.
        let frames = (max_packet_bytes - 4) / usize::from(tracks);
        let pcm_samples = frames
            .checked_mul(2)
            .ok_or(Error::Limit("audio PCM size"))?;
        if pcm_samples > isize::MAX as usize / core::mem::size_of::<i16>() {
            return Err(Error::Limit("audio PCM size"));
        }
        Ok(Self {
            sample_rate: header.audio_sample_rate,
            tracks,
            track,
            packets: header.audio_packets,
            requirements: AudioBufferRequirements {
                packet_bytes: max_packet_bytes,
                pcm_samples,
            },
        })
    }

    /// Declared playback sample rate in Hz. No rate conversion is performed.
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// Number of interleaved output channels (two, in left/right order).
    pub const fn channels(self) -> u16 {
        2
    }

    /// Number of available audio tracks.
    pub const fn tracks(self) -> u16 {
        self.tracks
    }

    /// Selected one-based audio track.
    pub const fn track(self) -> u16 {
        self.track
    }

    /// Declared number of compressed audio packets.
    pub const fn packets(self) -> u32 {
        self.packets
    }

    /// Storage sizes derived solely from the file header. This never allocates.
    ///
    /// `pcm_samples` counts **interleaved `i16` elements**, including both channels,
    /// not bytes or stereo frames. This conservative upper bound covers every
    /// supported packet consistent with the header, independently of movie length.
    /// Resource limits do not change the result; constructors reject requirements
    /// exceeding their limits. Use [`Self::validate_limits`] to apply a policy
    /// before allocating storage yourself, if desired.
    pub const fn buffer_requirements(self) -> AudioBufferRequirements {
        self.requirements
    }

    /// Check whether this file's requirements fit an application's resource policy.
    ///
    /// Constructors perform this check automatically. Call it separately if you
    /// want to reject oversized requirements before allocating your own buffers.
    /// This does not allocate or change the requirements. Either bound exceeding
    /// its limit returns [`Error::Limit`], even if an actual packet might be smaller.
    pub fn validate_limits(self, limits: AudioLimits) -> Result<()> {
        if self.requirements.packet_bytes > limits.max_packet_bytes {
            return Err(Error::Limit("audio packet bytes"));
        }
        if self.requirements.pcm_samples / 2 > limits.max_frames_per_packet {
            return Err(Error::Limit("audio samples per packet"));
        }
        Ok(())
    }
}

/// Reusable storage sufficient for one selected audio track, derived from its header.
///
/// Obtain this from [`AudioInfo::buffer_requirements`] before constructing a
/// decoder. Both sizes are per packet; neither grows with movie duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioBufferRequirements {
    /// Compressed bytes, including the sample count and all tracks' data, excluding
    /// the eight-byte container packet header. Only custom streaming I/O needs this
    /// workspace; [`SliceAudioDecoder`] borrows compressed bytes directly.
    pub packet_bytes: usize,
    /// Interleaved PCM16 elements (`i16`, **not bytes**), including both channels.
    /// This is a conservative capacity; each decode returns only the used prefix.
    pub pcm_samples: usize,
}

/// Optional resource policy, enforced when constructing a decoder.
///
/// These caps accept or reject the file's declared requirements; they never
/// change [`AudioInfo::buffer_requirements`] or trim output to fit.
#[derive(Debug, Clone, Copy)]
pub struct AudioLimits {
    /// Maximum declared compressed packet bytes. Checked before construction. Default 1 MiB.
    pub max_packet_bytes: usize,
    /// Maximum permitted header-derived bound in stereo sample frames (two PCM
    /// elements per frame). Checked during construction, using the conservative
    /// buffer requirement rather than scanning packets. Default 1 Mi frames.
    pub max_frames_per_packet: usize,
}

impl Default for AudioLimits {
    fn default() -> Self {
        Self {
            max_packet_bytes: 1024 * 1024,
            max_frames_per_packet: 1024 * 1024,
        }
    }
}

/// Coding mode of an HVQM4 IMA audio packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum AudioPacketMode {
    /// Continue the predictor state from the previous packet (wire value 2).
    Continue = 2,
    /// Initialize both channel predictors, emitting their samples (wire value 3).
    Initialize = 3,
}

impl TryFrom<u16> for AudioPacketMode {
    type Error = Error;

    fn try_from(value: u16) -> Result<Self> {
        match value {
            2 => Ok(Self::Continue),
            3 => Ok(Self::Initialize),
            _ => Err(Error::Invalid("unsupported IMA packet mode")),
        }
    }
}

/// Allocation-free decoder for demuxed HVQM4 IMA-ADPCM packets.
///
/// Stores only container metadata and two channel predictors. Output retains the
/// codec's L/R order. Mode 3 emits the initial predictor; mode 2 continues the
/// selected track's state. Each instance selects one explicitly one-based track.
pub struct AudioPacketDecoder {
    info: AudioInfo,
    state: [Predictor; 2],
    initialized: bool,
}

impl AudioPacketDecoder {
    /// Initialize predictor state from parsed audio metadata with default limits.
    /// No allocation occurs. Decode mode 3 before submitting mode 2 packets.
    pub fn new(info: AudioInfo) -> Result<Self> {
        Self::with_limits(info, AudioLimits::default())
    }

    /// Apply limits and initialize predictor state without allocating.
    pub fn with_limits(info: AudioInfo, limits: AudioLimits) -> Result<Self> {
        info.validate_limits(limits)?;
        Ok(Self {
            info,
            state: [Predictor::default(); 2],
            initialized: false,
        })
    }

    /// Selected audio format.
    pub fn metadata(&self) -> AudioInfo {
        self.info
    }

    /// Header-derived storage requirements, identical to those on its metadata.
    pub fn buffer_requirements(&self) -> AudioBufferRequirements {
        self.info.buffer_requirements()
    }

    /// Decode into caller-owned interleaved PCM16 storage, returning the used part.
    ///
    /// `packet` starts at the four-byte sample count, after the eight-byte
    /// container packet header; pass its mode (2 or 3) separately. It includes all
    /// tracks, but only the selected track is decoded. Begin with mode 3.
    ///
    /// All validation precedes decoding. Errors leave both predictor state and
    /// output unchanged, so a small buffer can be replaced and the packet retried.
    /// Elements beyond the returned slice are untouched. No allocation occurs.
    pub fn decode_into<'a>(
        &mut self,
        mode: AudioPacketMode,
        packet: &[u8],
        pcm: &'a mut [i16],
    ) -> Result<&'a [i16]> {
        if packet.len() > self.info.requirements.packet_bytes {
            return Err(Error::Limit("audio packet"));
        }
        let samples = usize::try_from(be32(packet, 0)?)
            .map_err(|_| Error::Limit("audio samples per packet"))?;
        if samples == 0 || samples > self.info.requirements.pcm_samples / 2 {
            return Err(Error::Limit("audio samples per packet"));
        }
        if mode == AudioPacketMode::Continue && !self.initialized {
            return Err(Error::Invalid("IMA continuation before initialization"));
        }
        let data = &packet[4..];
        let tracks = usize::from(self.info.tracks);
        if !data.len().is_multiple_of(tracks) {
            return Err(Error::Invalid("audio track partition"));
        }
        let track_size = data.len() / tracks;
        let prefix = if mode == AudioPacketMode::Initialize {
            6
        } else {
            0
        };
        // Mode 3 carries one unused final byte after the predictor sample.
        if track_size != samples + prefix {
            return Err(Error::Invalid("audio sample count disagrees with packet"));
        }
        let at = (usize::from(self.info.track) - 1) * track_size;
        let track = &data[at..at + track_size];
        let mut state = self.state;
        let start = if mode == AudioPacketMode::Initialize {
            for (channel, predictor) in state.iter_mut().enumerate() {
                let at = (1 - channel) * 3;
                let index = track[at + 2];
                if index > 88 {
                    return Err(Error::Invalid("IMA step index"));
                }
                *predictor = Predictor {
                    sample: i32::from(i16::from_be_bytes([track[at], track[at + 1]])),
                    index: i32::from(index),
                };
            }
            1
        } else {
            0
        };
        let count = samples * 2; // Checked upper bound at construction.
        let provided = pcm.len();
        let pcm = pcm.get_mut(..count).ok_or(Error::BufferTooSmall {
            buffer: BufferKind::AudioPcm,
            required: count,
            provided,
        })?;
        if start == 1 {
            pcm[0] = state[0].sample as i16;
            pcm[1] = state[1].sample as i16;
        }
        for (i, &byte) in track[prefix..prefix + samples - start].iter().enumerate() {
            pcm[(i + start) * 2] = state[0].nibble(byte & 15);
            pcm[(i + start) * 2 + 1] = state[1].nibble(byte >> 4);
        }
        self.state = state;
        self.initialized = true;
        Ok(pcm)
    }
}

/// Allocation-free H4M audio reader over a byte slice and caller-owned PCM storage.
///
/// Video packets are skipped without copying. Storage can be an inline array or
/// any `AsMut<[i16]>`, including a borrowed slice. Query
/// [`AudioInfo::buffer_requirements`] before providing storage. Construction
/// rejects smaller buffers; the decoder never allocates a fallback. Custom
/// storage must continue to expose at least that many elements on every call.
pub struct SliceAudioDecoder<'input, P> {
    inner: AudioContainer<SliceSource<'input>>,
    pcm: P,
}

impl<'input, P: AsMut<[i16]>> SliceAudioDecoder<'input, P> {
    /// Attach caller-owned PCM storage with default limits, without allocating.
    pub fn with_buffer(input: &'input [u8], track: u16, pcm: P) -> Result<Self> {
        Self::with_buffer_and_limits(input, track, pcm, AudioLimits::default())
    }

    /// Validate the header and select a one-based track using caller-owned PCM storage.
    ///
    /// Validate storage against [`AudioInfo::buffer_requirements`] immediately.
    /// Returns [`Error::BufferTooSmall`] before consuming packets or modifying
    /// PCM if storage is insufficient, even if the first packet would fit.
    /// Pass a borrowed slice to retain ownership of storage when construction fails.
    /// No allocation occurs during construction or decoding.
    pub fn with_buffer_and_limits(
        input: &'input [u8],
        track: u16,
        mut pcm: P,
        limits: AudioLimits,
    ) -> Result<Self> {
        let header = Header::parse(input)?;
        let info = header.audio_info(track)?;
        let codec = AudioPacketDecoder::with_limits(info, limits)?;
        let required = codec.buffer_requirements().pcm_samples;
        let provided = pcm.as_mut().len();
        if provided < required {
            return Err(Error::BufferTooSmall {
                buffer: BufferKind::AudioPcm,
                required,
                provided,
            });
        }
        Ok(Self {
            inner: AudioContainer::new(
                SliceSource {
                    remaining: &input[Header::SIZE..],
                },
                header,
                codec,
            ),
            pcm,
        })
    }

    /// Selected audio format.
    pub fn metadata(&self) -> AudioInfo {
        self.inner.codec.metadata()
    }

    /// Container metadata, without constructing a video decoder.
    pub fn header(&self) -> &Header {
        &self.inner.header
    }

    /// Storage requirements checked at construction, in compressed bytes and PCM elements.
    pub fn buffer_requirements(&self) -> AudioBufferRequirements {
        self.inner.codec.buffer_requirements()
    }

    /// Decode a packet into borrowed interleaved PCM16. Errors poison the decoder.
    pub fn next_block(&mut self) -> Result<Option<&[i16]>> {
        self.inner.next_block(self.pcm.as_mut())
    }

    /// Recover the unconsumed input and caller-owned PCM storage.
    /// Trailing bytes after the declared container body are left unread.
    pub fn into_inner(self) -> (&'input [u8], P) {
        (self.inner.source.remaining, self.pcm)
    }
}

/// Streaming, bounded-memory stereo decoder, available with `std` only.
///
/// Uses the same allocation-free packet decoder and container parser as
/// [`SliceAudioDecoder`], with two buffers allocated once at construction.
/// Video is skipped without allocating video frames.
#[cfg(feature = "std")]
pub struct AudioDecoder<R> {
    inner: AudioContainer<ReaderSource<R>>,
    pcm: Vec<i16>,
}

#[cfg(feature = "std")]
impl<R: Read> AudioDecoder<R> {
    /// Open a stream and explicitly select a one-based audio track.
    pub fn new(reader: R, track: u16) -> Result<Self> {
        Self::with_limits(reader, track, AudioLimits::default())
    }

    /// Validate the profile and allocate bounded packet/PCM storage.
    pub fn with_limits(mut reader: R, track: u16, limits: AudioLimits) -> Result<Self> {
        let mut bytes = [0; Header::SIZE];
        reader.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes)?;
        let info = header.audio_info(track)?;
        let codec = AudioPacketDecoder::with_limits(info, limits)?;
        let requirements = codec.buffer_requirements();
        let packet = crate::storage::filled_vec(requirements.packet_bytes, 0)?;
        let pcm = crate::storage::filled_vec(requirements.pcm_samples, 0)?;
        Ok(Self {
            inner: AudioContainer::new(ReaderSource { reader, packet }, header, codec),
            pcm,
        })
    }

    /// Selected audio format.
    pub fn metadata(&self) -> AudioInfo {
        self.inner.codec.metadata()
    }

    /// Container metadata, without constructing a video decoder.
    pub fn header(&self) -> &Header {
        &self.inner.header
    }

    /// Recover the input reader.
    pub fn into_inner(self) -> R {
        self.inner.source.reader
    }

    /// Decode a packet into borrowed interleaved PCM16. Errors poison the decoder.
    pub fn next_block(&mut self) -> Result<Option<&[i16]>> {
        self.inner.next_block(&mut self.pcm)
    }
}

trait AudioSource {
    fn header<const N: usize>(&mut self) -> Result<[u8; N]>;
    fn packet(&mut self, size: usize) -> Result<&[u8]>;
    fn skip(&mut self, size: usize) -> Result<()>;
}

struct SliceSource<'input> {
    remaining: &'input [u8],
}

impl AudioSource for SliceSource<'_> {
    fn header<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0; N];
        bytes.copy_from_slice(self.packet(N)?);
        Ok(bytes)
    }

    fn packet(&mut self, size: usize) -> Result<&[u8]> {
        let (packet, rest) = self
            .remaining
            .split_at_checked(size)
            .ok_or(Error::Truncated)?;
        self.remaining = rest;
        Ok(packet)
    }

    fn skip(&mut self, size: usize) -> Result<()> {
        self.packet(size)?;
        Ok(())
    }
}

#[cfg(feature = "std")]
struct ReaderSource<R> {
    reader: R,
    packet: Vec<u8>,
}

#[cfg(feature = "std")]
impl<R: Read> AudioSource for ReaderSource<R> {
    fn header<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0; N];
        self.reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn packet(&mut self, size: usize) -> Result<&[u8]> {
        let bytes = self
            .packet
            .get_mut(..size)
            .ok_or(Error::Limit("audio packet"))?;
        self.reader.read_exact(bytes)?;
        Ok(bytes)
    }

    fn skip(&mut self, size: usize) -> Result<()> {
        let mut left = size;
        let mut scratch = [0; 4096];
        while left != 0 {
            let n = left.min(scratch.len());
            self.reader.read_exact(&mut scratch[..n])?;
            left -= n;
        }
        Ok(())
    }
}

struct AudioContainer<S> {
    source: S,
    header: Header,
    codec: AudioPacketDecoder,
    body: u32,
    blocks: u32,
    block_bytes: u32,
    block_audio: u32,
    block_video: u32,
    audio: u32,
    video: u32,
    failed: bool,
}

impl<S: AudioSource> AudioContainer<S> {
    fn new(source: S, header: Header, codec: AudioPacketDecoder) -> Self {
        Self {
            source,
            body: header.body_size,
            header,
            codec,
            blocks: 0,
            block_bytes: 0,
            block_audio: 0,
            block_video: 0,
            audio: 0,
            video: 0,
            failed: false,
        }
    }

    fn next_block<'a>(&mut self, pcm: &'a mut [i16]) -> Result<Option<&'a [i16]>> {
        if self.failed {
            return Err(Error::Failed);
        }
        self.failed = true;
        let count = self.decode_next(pcm)?;
        self.failed = false;
        Ok(count.map(|n| &pcm[..n]))
    }

    fn decode_next(&mut self, pcm: &mut [i16]) -> Result<Option<usize>> {
        let header = self.header;
        loop {
            if self.block_audio == 0 && self.block_video == 0 {
                if self.block_bytes != 0 {
                    return Err(Error::Invalid("audio block byte count"));
                }
                if self.blocks == header.blocks {
                    if !matches!(self.body, 0 | 16)
                        || self.audio != header.audio_packets
                        || self.video != header.video_frames
                    {
                        return Err(Error::Invalid("audio container totals"));
                    }
                    // GQSEAF stores a 16-byte opaque trailer inside body_size.
                    if self.body == 16 {
                        self.source.skip(16)?;
                        self.body = 0;
                    }
                    return Ok(None);
                }
                self.body = self
                    .body
                    .checked_sub(20)
                    .ok_or(Error::Invalid("block exceeds body"))?;
                let b = self.source.header::<20>()?;
                if be32(&b, 16)? != 0x01000000 {
                    return Err(Error::Invalid("audio block marker"));
                }
                self.block_bytes = be32(&b, 4)?;
                self.block_video = be32(&b, 8)?;
                self.block_audio = be32(&b, 12)?;
                if self.block_bytes > self.body
                    || self.block_video > header.video_frames - self.video
                    || self.block_audio > header.audio_packets - self.audio
                {
                    return Err(Error::Invalid("block exceeds declared totals"));
                }
                self.blocks += 1;
                continue;
            }
            if self.block_bytes < 8 {
                return Err(Error::Invalid("packet header exceeds block"));
            }
            let ph = self.source.header::<8>()?;
            let size = be32(&ph, 4)?;
            let total = size
                .checked_add(8)
                .ok_or(Error::Invalid("packet size overflow"))?;
            self.block_bytes = self
                .block_bytes
                .checked_sub(total)
                .ok_or(Error::Invalid("packet exceeds block"))?;
            self.body = self
                .body
                .checked_sub(total)
                .ok_or(Error::Invalid("packet exceeds body"))?;
            let mode = be16(&ph, 2)?;
            match be16(&ph, 0)? {
                1 => {
                    crate::FrameType::try_from(mode)?;
                    self.block_video = self
                        .block_video
                        .checked_sub(1)
                        .ok_or(Error::Invalid("too many video packets"))?;
                    if size > header.max_video_packet_size {
                        return Err(Error::Invalid("video packet exceeds maximum"));
                    }
                    self.source.skip(size as usize)?;
                    self.video += 1;
                }
                0 => {
                    self.block_audio = self
                        .block_audio
                        .checked_sub(1)
                        .ok_or(Error::Invalid("too many audio packets"))?;
                    if size as usize > self.codec.info.requirements.packet_bytes {
                        return Err(Error::Limit("audio packet"));
                    }
                    let bytes = self.source.packet(size as usize)?;
                    let count = self.codec.decode_into(mode.try_into()?, bytes, pcm)?.len();
                    self.audio += 1;
                    return Ok(Some(count));
                }
                _ => return Err(Error::Invalid("unknown audio container packet")),
            }
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Predictor {
    sample: i32,
    index: i32,
}

impl Predictor {
    fn nibble(&mut self, code: u8) -> i16 {
        const STEP: [i32; 89] = [
            7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55,
            60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307,
            337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411,
            1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358,
            5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500,
            20350, 22385, 24623, 27086, 29794, 32767,
        ];
        const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
        let step = STEP[self.index as usize];
        let mut delta = step >> 3;
        if code & 1 != 0 {
            delta += step >> 2;
        }
        if code & 2 != 0 {
            delta += step >> 1;
        }
        if code & 4 != 0 {
            delta += step;
        }
        if code & 8 != 0 {
            delta = -delta;
        }
        self.sample = (self.sample + delta).clamp(-32768, 32767);
        self.index = (self.index + INDEX[usize::from(code & 7)]).clamp(0, 88);
        self.sample as i16
    }
}
