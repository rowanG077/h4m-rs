# h4m

A pure Rust HVQM4 1.3/1.5 (`.h4m`) video and IMA-ADPCM audio decoder. No production
dependencies or unsafe code. Both decoding cores work without `std` or `alloc`;
callers can supply all frame, descriptor, and PCM storage.

Video supports I/P/B pictures and 4:2:0, 4:2:2, and 4:4:4 sampling. Dimensions must
be nonzero multiples of eight. Audio supports stereo IMA (format 2, 16-bit,
flags 0), predictor initialization (mode 3), and continuation (mode 2), at declared
rates of 8,000–192,000 Hz. Audio track selection is explicit and **one-based**.
Unsupported formats return errors. The library preserves channel order and
sample rate; it does not resample, swap channels, or trim audio.

## Choose a decoding layer

| Input           | Video                | Audio                | Requirements        |
| --------------- | -------------------- | -------------------- | ------------------- |
| Demuxed packets | `VideoPacketDecoder` | `AudioPacketDecoder` | No `std` or `alloc` |
| H4M byte slice  | `SliceVideoDecoder`  | `SliceAudioDecoder`  | No `std` or `alloc` |
| `std::io::Read` | `VideoDecoder`       | `AudioDecoder`       | `std` feature       |

All decoders expose `metadata()`. Container decoders additionally expose
`header()`. Video decoders skip audio; audio decoders skip video. Output borrows
reusable storage: consume or copy it before requesting the next frame/block.

| Cargo features                    | Storage and helpers                                    |
| --------------------------------- | ------------------------------------------------------ |
| None (`default-features = false`) | Caller-owned buffers; RGB conversion into a slice      |
| `alloc`                           | Also owned video buffers and reusable RGB vectors      |
| `std` (default; includes `alloc`) | Also streaming readers, output helpers, extraction CLI |

Decoding never grows buffers. Streaming video makes five setup allocations;
streaming audio makes two. Allocation failures reported by the allocator return
`Error::Allocation`. These guarantees exclude caller I/O and output handling.
`Frame::to_rgb` may grow its vector; `to_rgb_into` never allocates.

## Determine storage before construction

`Header::parse(input)` examines the first `Header::SIZE` bytes (68). Its fields are
immutable and available through accessors. Use `header.video()` for validated
`VideoInfo`, and `header.audio_info(track)?` for validated `AudioInfo`. Alternatively,
`AudioInfo::parse(input, track)?` performs both steps for audio-only callers.
Metadata parsing does not decode packets or validate the entire file.

`buffer_requirements()` takes **no limits**, does not allocate, and cannot fail:
sizes follow from validated metadata. They are independent of movie duration.

| Requirement                             | Unit and meaning                                                                                                                     |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `VideoBufferRequirements::frame_bytes`  | Bytes in **each** of three Y/U/V frame buffers                                                                                       |
| `VideoBufferRequirements::block_states` | `BlockState` elements in the workspace                                                                                               |
| `AudioBufferRequirements::pcm_samples`  | Interleaved `i16` elements, including **both channels**, for one selected track                                                      |
| `AudioBufferRequirements::packet_bytes` | Compressed bytes, including the sample count and all tracks, excluding the eight-byte packet header; useful for custom streaming I/O |

Audio requirements are conservative per-packet bounds calculated from the
header. Supplying exactly `pcm_samples` elements is sufficient for any supported
packet consistent with that header. Slice readers borrow compressed input and
do not need a packet buffer. Each audio decode returns only the used output
prefix and leaves the remainder untouched.

`VideoInfo::rgb_buffer_size()` and `Frame::rgb_buffer_size()` give packed RGB24
output bytes. `Frame` and its `Plane` views are immutable and tightly packed.
To wrap externally decoded planes, use `Frame::from_planes`; it checks exact
lengths against the layout before constructing a frame. Padded strides are not
supported.

## Decode audio with caller-owned storage

```toml
h4m = { path = "/path/to/h4m-rs", default-features = false }
```

For a fixed 8 KiB PCM budget, the following works without an allocator:

```rust
use h4m::{Error, SliceAudioDecoder};

fn decode_audio(input: &[u8], track: u16) -> Result<(), Error> {
    let mut pcm = [0i16; 4096];
    let mut decoder = SliceAudioDecoder::with_buffer(input, track, &mut pcm[..])?;
    while let Some(samples) = decoder.next_block()? {
        // Consume this packet's interleaved left/right PCM16.
    }
    Ok(())
}
```

To allocate precisely on the caller's side:

```rust
use h4m::{AudioInfo, AudioLimits, Error, SliceAudioDecoder};

fn decode_audio(input: &[u8], track: u16) -> Result<(), Error> {
    let info = AudioInfo::parse(input, track)?;
    let required = info.buffer_requirements();
    let limits = AudioLimits::default();
    info.validate_limits(limits)?; // Optional policy check before allocating.
    let mut pcm = vec![0i16; required.pcm_samples];
    let mut decoder =
        SliceAudioDecoder::with_buffer_and_limits(input, track, &mut pcm[..], limits)?;
    while let Some(samples) = decoder.next_block()? {
        // Consume samples before requesting the next block.
    }
    Ok(())
}
```

Construction rejects insufficient storage immediately with
`Error::BufferTooSmall { buffer, required, provided }`; counts use the units in
the table above. This happens even if the first audio packet would fit.
Borrow buffers when you need to retain them on constructor failure. Moving an
array or other storage handle transfers ownership. `into_inner()` recovers a
slice reader's unconsumed input and storage.

For demuxed audio, construct `AudioPacketDecoder::new(info)?`, then call
`decode_into(AudioPacketMode::Initialize, packet, pcm)` for initialization and
`AudioPacketMode::Continue` for subsequent packets. Convert wire codes with
`AudioPacketMode::try_from(code)?`. Include the four-byte sample count and every
track's data; exclude the eight-byte container packet header. The packet API
accepts any PCM slice large enough for that particular packet. Predictor state
and output remain unchanged on error, allowing a failed packet to be retried.

## Decode video with caller-owned storage

Size storage from `Header::parse(input)?.video().buffer_requirements()`.
Buffers may live in an arena, static storage, or on the stack:

```rust
use h4m::{BlockState, Error, SliceVideoDecoder, VideoBuffers};

fn decode_video(
    input: &[u8],
    frames: [&mut [u8]; 3],
    blocks: &mut [BlockState],
    rgb: &mut [u8],
) -> Result<(), Error> {
    let mut decoder = SliceVideoDecoder::with_buffers(input, VideoBuffers { frames, blocks })?;
    while let Some(frame) = decoder.next_frame()? {
        frame.to_rgb_into(rgb)?;
        // Consume this frame before decoding the next one.
    }
    Ok(())
}
```

For demuxed video use `VideoPacketDecoder::with_buffers(info, buffers)?`, then
`decode(kind, display_index, packet)`. Omit the four-byte display index from the
payload and begin with an I picture. With `alloc`, `VideoPacketDecoder::new(info)`
allocates storage. `OwnedVideoPacketDecoder` and `BorrowedVideoPacketDecoder<'a>`
name vector-backed and slice-backed storage types.

Video buffers are retained exclusively, preventing replacement of live reference
pictures. `into_buffers()` recovers packet-decoder buffers in their original order.
Reference rotation changes indices without copying frames, including inline
arrays. Custom frame storage implements `AsRef<[u8]>` and `AsMut<[u8]>`; workspace
implements `AsMut<[BlockState]>`. Views must expose the same stable storage and
at least the required prefix throughout decoding. Audio storage implements
`AsMut<[i16]>`. Undersized views return errors, never unchecked indexing panics.

## Stream with standard I/O

Wrap files in `BufReader` for efficient small reads:

```rust,no_run
# #[cfg(feature = "std")]
# fn example() -> Result<(), Box<dyn std::error::Error>> {
use std::{fs::File, io::BufReader};
let mut video = h4m::VideoDecoder::new(BufReader::new(File::open("movie.h4m")?))?;
let mut rgb = Vec::new();
while let Some(frame) = video.next_frame()? {
    frame.to_rgb(&mut rgb)?;
    println!("presentation index: {}", frame.display_index());
    // frame.planes() returns Y, U, V views with data(), width(), and height().
}
let mut audio = h4m::AudioDecoder::new(BufReader::new(File::open("movie.h4m")?), 2)?;
while let Some(pcm) = audio.next_block()? {
    // Consume the selected track's left/right PCM16 samples.
}
# Ok(())
# }
```

Video arrives in **decoding order**, which may differ from presentation order
(e.g. `I0, P3, B1, B2`). Use `display_index()` to reorder it or calculate timestamps
with `header.microseconds_per_frame()`. Copy retained pictures in the application.
Decoders have independent state and may run on separate threads.

## Limits and errors

Default constructors use default limits. To set an acceptance policy explicitly,
use `with_limits`, `with_buffer_and_limits`, or `with_buffers_and_limits`.
Limits reject excessive requirements; they never reduce buffer sizes or output.
Call `info.validate_limits(limits)?` before allocating caller-owned storage if
desired. `Header::validate_video_limits` additionally checks the declared packet
bound before allocating custom streaming storage.

- `VideoLimits`: default 16,777,216 luma pixels and 64 MiB compressed picture
  payload. `max_frame_bytes` excludes the four-byte display index and eight-byte
  packet header, consistently in packet and container APIs.
- `AudioLimits`: default 1 MiB compressed packet bytes and 1 Mi stereo frames per
  packet. `max_frames_per_packet` counts stereo frames, **two PCM elements each**.
  Constructors check the complete header-derived bound before decoding.

Malformed input, unsupported profiles, truncation, and exceeded limits return
errors. Audio packet errors preserve state/output. Video packet errors invalidate
the decoder because reference pictures may have changed. All container decode
errors invalidate the reader because input may have been consumed; later decode
calls return `Error::Failed`. Reconstruct an invalidated decoder to restart.
Successful end-of-stream calls keep returning `None`.

Video readers enforce block sizes, packet maxima and frame counts, and leave data
after the last declared block unread. The declared body size is exposed but not
enforced by video readers, matching the reference container behavior. Audio
readers additionally validate the declared body and consume an optional 16-byte
body trailer; bytes after that declared body remain unread. `into_inner()` returns
the reader at its current position, without rewinding or implicit buffering.

## Extract and verify

```sh
cargo install --path . --locked
h4m movie.h4m frames
h4m movie.h4m planes --yuv
cargo run --release --example audio -- movie.h4m 2 track2.pcm
```

Video filenames use presentation indices. YUV files contain native tightly packed
Y/U/V planes; audio output is raw little-endian PCM16. Existing output files are
never overwritten. RGB uses the reference's full-range coefficients and truncation,
with conversion for all three supported chroma layouts.

```sh
nix develop
cargo fmt --all -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
taplo fmt --check Cargo.toml fuzz/Cargo.toml
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
cargo test --locked --no-default-features --features alloc
./scripts/test-reference.sh
./scripts/test-reference.sh --release
H4M_ORIGINALS=/path/to/extracted/discs ./scripts/test-originals.sh
cargo bench --bench decode
nix flake check
nix build
```

Development uses stable Rust from Fenix and nixpkgs, pinned in `flake.lock`.
Video reference tools pin `mbcgh/h4m-video-decoder` at
`02d66526e62e346c3ef7f92eac49cc07356a2c62`. The opt-in original-movie test compares
every plane byte and presentation index. Both tracks of all 11 local GQSEAF movies
match pinned vgmstream r2117 PCM16 references. The sibling Resonance repository's
`tools/decoder-reference` records corpus hashes and comparisons.

Synthetic tests exercise all feature configurations, buffer boundaries, predictor
continuity, track isolation, malformed input and error recovery. `fuzz/` contains a
bounded libFuzzer target. Original assets stay local; production has no dependencies.
See [the API review](docs/api-review.md) for findings and verification scope, and
[RELEASING.md](RELEASING.md) for releases. Pre-1.0 API changes are made directly,
without deprecated aliases or compatibility adapters.

## License

**LGPL-2.0-or-later**; see [LICENSE](./LICENSE). Based on
[Tilka's HVQM4 decoder](https://github.com/Tilka/hvqm4), via the
[mbcgh fork](https://github.com/mbcgh/h4m-video-decoder), retaining its original
license. Upstream credits Tilka and flacs/hcs (container/audio).
IMA arithmetic and framing were checked against vgmstream r2117; its copyright
and permissive license are retained in [LICENSE.vgmstream](LICENSE.vgmstream).
Rust rewrite and synthetic tests by h4m contributors, 2026.
