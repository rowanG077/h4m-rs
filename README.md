# h4m

A pure Rust HVQM4 1.3/1.5 (`.h4m`) video decoder with a library and frame
extraction CLI. No dependencies or unsafe code. The decoding core is `no_std`
and needs no allocator; callers can provide all frame and descriptor buffers.

Supports I, P, and B pictures, adaptive orthogonal transforms, literal blocks,
weighted DC reconstruction, run-length/Huffman coding, half-pixel motion, and
4:2:0, 4:2:2, and 4:4:4 sampling. Audio packets are skipped. Dimensions must be
nonzero multiples of eight. Other HVQM versions and chroma layouts are rejected.

## Features and storage

| Cargo features | Available APIs | Allocation behavior |
|---|---|---|
| None (`default-features = false`) | `VideoDecoder`, `SliceDecoder`, `Header`, RGB conversion into a slice | No `std` or `alloc`; caller-owned storage |
| `alloc` | Also `OwnedVideoDecoder::new` and reusable RGB vectors | Frame/descriptor storage allocated at construction |
| `std` (default, includes `alloc`) | Also `Decoder<R: Read>`, file-writing helpers, CLI | Five setup allocations in the streaming decoder; none while decoding |

`VideoInfo::buffer_requirements()` reports bytes for **each** of three frame
buffers and the element count for a `BlockState` workspace. Borrowed buffers are
kept exclusively by the decoder, preventing replacement of live reference data.
They can be recovered in their original order with `into_buffers()`.
Reference rotation changes indices without copying frame data, including when
buffers are inline arrays.
Huffman codebooks, transforms, and the DC nest use bounded inline/stack storage.
No unsafe aliasing, uninitialized buffers, or per-frame heap scratch is used.
The allocation guarantees exclude the caller's I/O implementation and output
handling; `Frame::to_rgb` may grow its vector, while `to_rgb_into` never allocates.

## Decode without an allocator

```toml
h4m = { path = "/path/to/h4m-rs", default-features = false }
```

The following function works in `no_std` code. The caller supplies storage sized
from `Header::parse(input)?.video.buffer_requirements()` and an RGB output buffer
of `width * height * 3` bytes. Buffers may live in an arena, static storage, or on
the stack. No allocator is needed by the decoder or container parser.

```rust
use h4m::{BlockState, DecoderBuffers, Error, Limits, SliceDecoder};

fn decode(input: &[u8], frames: [&mut [u8]; 3], blocks: &mut [BlockState],
          rgb: &mut [u8]) -> Result<(), Error> {
    let mut decoder = SliceDecoder::with_buffers(
        input, DecoderBuffers { frames, blocks }, Limits::default(),
    )?;
    while let Some(frame) = decoder.next_frame()? {
        frame.to_rgb_into(rgb)?;
        // Consume this frame before decoding the next one.
    }
    Ok(())
}
```

For already demuxed packets, use `VideoDecoder::with_buffers` with the same
`DecoderBuffers`. `BorrowedVideoDecoder<'a>` is an alias for slice-backed storage;
`OwnedVideoDecoder` is the allocating convenience alias. Custom storage can
implement `AsRef<[u8]>`/`AsMut<[u8]>` for frames and `AsMut<[BlockState]>` for
workspace; the frame views must refer to the same stable storage.

## Stream a file with standard I/O

```rust,no_run
use std::{fs::File, io::BufReader};
use h4m::Decoder;

let mut decoder = Decoder::new(BufReader::new(File::open("movie.h4m")?))?;
println!("{:?}", decoder.header());
let mut rgb = Vec::new();
while let Some(frame) = decoder.next_frame()? {
    // Borrowed, tightly packed planes: frame.y, frame.u, frame.v.
    // Copy pixels here if you need to retain them after the next decode call.
    frame.to_rgb(&mut rgb);
    println!("presentation index: {}", frame.display_index);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Decoder<R: Read>` streams the container and reuses its packet and three frame
buffers. Frames arrive in **decoding order**, which can differ from presentation
order (`I0, P3, B1, B2`, for example). Use `display_index` to order frames or
calculate timestamps (`index * header.microseconds_per_frame`). For playback,
copy/reorder frames in your application. The CLI names frames by presentation index.

`VideoDecoder` accepts demuxed packets: pass the frame type and display index
separately, and omit the packet's initial four-byte display ID. Start with an I
frame. Decoder instances are independent and can run on separate threads.

`Decoder::with_limits` and `VideoDecoder::with_limits` accept `Limits`; defaults
are 16,777,216 luma pixels and 64 MiB per compressed frame. Malformed data returns
an error; construct a new decoder to continue.
Container body-size metadata is exposed but not enforced, matching the reference;
individual block sizes, frame counts, and the declared maximum packet size are
checked. The latter bounds the streaming decoder's one packet allocation. Trailing data after the
last declared block is left unread.

## Migration from 0.2.x

This refactor changes the public API and is intended for **0.3.0**. Release
versioning is still performed by the tag-triggered release workflow.

- Replace `VideoInfo { ... }` with
  `VideoInfo::new(version, width, height, ChromaSampling::Yuv420)?` (or `Yuv422` /
  `Yuv444`). Dimensions and sampling are validated once. Use `width()`, `height()`,
  `version()`, and `sampling()` to inspect the format.
- Explicit decoder type annotations use `OwnedVideoDecoder` for allocated
  storage or `BorrowedVideoDecoder<'a>` for caller-owned slices. Construction via
  `VideoDecoder::new(info)` still infers owned storage when `alloc` is enabled.
- Small output/workspace buffers return `Error::BufferTooSmall` with the required
  and supplied sizes. The decoder never silently allocates a replacement buffer.
- The standard streaming constructor checks the header's maximum packet size
  against `Limits` before allocating. Packets exceeding that declared maximum
  are rejected instead of growing the packet buffer during decoding.

## Extract frames

```sh
cargo install --path . --locked
h4m movie.h4m frames
h4m movie.h4m planes --yuv
```

The default output is `frame_0000000000.ppm`, etc. `--yuv` writes one tightly
packed Y/U/V file per frame, using the file's native chroma sampling. Existing
output files are not overwritten. RGB conversion reproduces the reference's
full-range coefficients and truncation; the reference CLI's RGB output supports
only 4:2:0, while this CLI also converts 4:2:2 and 4:4:4 correctly.

## Develop and test

Development and CI use stable Rust from Fenix and nixpkgs unstable, pinned in
`flake.lock`. Update both with `nix flake update nixpkgs fenix`.

```sh
nix develop                     # Fenix Rust tools and the reference decoder
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
cargo test --locked --no-default-features --features alloc
./scripts/test-reference.sh
./scripts/test-reference.sh --release
cargo bench --bench decode
nix flake check                 # Rust tests + debug/release reference equivalence
nix build                       # result/bin/h4m
nix build .#reference-decoder    # standalone C reference tools
```

Nix fetches [mbcgh/h4m-video-decoder](https://github.com/mbcgh/h4m-video-decoder)
at the fixed revision `02d66526e62e346c3ef7f92eac49cc07356a2c62`
as the reference decoder to test.

To compare a local collection of original movies, including every disc, run:

```sh
H4M_ORIGINALS=/path/to/extracted/discs nix develop -c ./scripts/test-originals.sh
```

This recursively discovers every `.h4m` file (case insensitive), checks every
Y/U/V byte and presentation index against the pinned C decoder, and verifies
frame counts. Both decoders stream frames, so it needs no decoded-movie files.
The test fails if the directory contains no movies. Original assets are supplied
locally and are never included in the repository or crate; this opt-in test is
separate from the synthetic reference comparisons run in CI.

Release instructions are in [RELEASING.md](RELEASING.md).

## License

Licensed under **LGPL-2.0-or-later**; see [LICENSE](LICENSE).
Based on [Tilka's HVQM4 decoder](https://github.com/Tilka/hvqm4), via the
[mbcgh fork](https://github.com/mbcgh/h4m-video-decoder), retaining its
[original license](https://github.com/mbcgh/h4m-video-decoder/blob/0970075/README.md#license).
Upstream credits Tilka and flacs/hcs (container/audio).
Rust rewrite and synthetic tests by h4m contributors, 2026.
