# Changelog

## Unreleased

## 0.4.0


### Added

- Stereo HVQM4 IMA-ADPCM audio decoding with explicit one-based track selection:
  `AudioPacketDecoder` and `SliceAudioDecoder` work without `std` or `alloc`;
  `AudioDecoder<R: Read>` provides streaming I/O. `AudioInfo` exposes metadata
  and buffer requirements before construction, and `AudioPacketMode` identifies
  predictor initialization and continuation. Memory use is bounded per packet.
- A raw PCM16 extraction example that refuses to overwrite existing files.
- `VideoInfo::rgb_buffer_size()` and `Frame::rgb_buffer_size()` for sizing RGB
  output, plus public `VideoInfo::validate_limits()` and
  `Header::validate_video_limits()` for checking resource limits before allocation.

### Changed

- Rename video types to distinguish packet decoding from container reading:

  | In 0.3.0               | New name                     |
  | ---------------------- | ---------------------------- |
  | `Decoder<R>`           | `VideoDecoder<R>`            |
  | `VideoDecoder<F, B>`   | `VideoPacketDecoder<F, B>`   |
  | `SliceDecoder`         | `SliceVideoDecoder`          |
  | `OwnedVideoDecoder`    | `OwnedVideoPacketDecoder`    |
  | `BorrowedVideoDecoder` | `BorrowedVideoPacketDecoder` |
  | `Limits`               | `VideoLimits`                |
  | `DecoderBuffers`       | `VideoBuffers`               |
  | `BufferRequirements`   | `VideoBufferRequirements`    |

- Video `with_buffers` constructors now use default limits. Use
  `with_buffers_and_limits` to pass an explicit policy. The packet decoder's
  `info()` becomes `metadata()`, also available on container decoders.
- Replace public `Header`, `Frame`, and `Plane` fields with read-only accessors.
  Use `Frame::from_planes()` to validate external planes before constructing a
  frame, preventing malformed plane lengths from causing RGB conversion panics.
  Header `audio_frames` becomes `audio_packets()`, and `max_frame_size` becomes
  `max_video_packet_size()`.
- Correct the header's `audio_channels` field to `audio_format()`: the encoded
  byte identifies the codec, not the channel count. Obtain validated channel
  counts through `Header::audio_info(track)?.channels()`.
- `Frame::to_rgb()` now returns `Result<(), Error>`. Allocating constructors and
  RGB conversion report allocator reservation failures as `Error::Allocation`.

### Fixed

- Apply `VideoLimits::max_frame_bytes` consistently to compressed picture payload,
  excluding the four-byte display index, in both packet and container decoders.
  Previously a container could reject a packet accepted with the same raw limit.
- Return buffer errors instead of panicking when custom video storage exposes
  undersized views after construction.


## 0.3.0

- Replace mutable numeric `VideoInfo` fields with a validated constructor,
  accessors, and `ChromaSampling`. Expose checked wire-code conversion for
  `FrameType`.
- Make `VideoDecoder` generic over owned or borrowed buffers. Add
  `DecoderBuffers`, `BlockState`, buffer requirements, and owned/borrowed aliases.
- Support `no_std` without `alloc`, with `alloc` and default `std` convenience
  layers. Add an allocation-free `SliceDecoder` and `Frame::to_rgb_into`.
- Preallocate the streaming packet buffer once; reject packets exceeding the
  declared maximum. Resource-limit failures can now occur at construction.
- Replace integer sentinels, packed block flags, and positional stream indices
  with typed syntax, named entropy streams, owned reference buffers, and explicit
  lifecycle states. Split geometry, motion sampling, and transforms into modules.


## 0.2.0

- Fix P pictures that select the current destination as their second motion
  reference, including reads from blocks already reconstructed in that picture.
  These valid streams previously failed with `future reference in P frame`.
- Add synthetic current-reference regressions and an opt-in streaming comparison
  of every original H4M movie under a supplied directory with the pinned C decoder.

## 0.1.0

- Pure Rust HVQM4 1.3/1.5 video decoding with I/P/B pictures and planar output.
- Streaming H4M demuxing with audio skipping, limits, and checked bitstreams.
- Borrowed frame API and PPM/native YUV extraction CLI.
- Deterministic C reference equivalence tests and malformed-input regression tests.
