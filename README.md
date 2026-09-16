# h4m

A pure Rust HVQM4 1.3/1.5 (`.h4m`) video decoder with a library and frame
extraction CLI. No dependencies or unsafe code.

Supports I, P, and B pictures, adaptive orthogonal transforms, literal blocks,
weighted DC reconstruction, run-length/Huffman coding, half-pixel motion, and
4:2:0, 4:2:2, and 4:4:4 sampling. Audio packets are skipped. Dimensions must be
nonzero multiples of eight. Other HVQM versions and chroma layouts are rejected.

## Use the library

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
individual block sizes and frame counts are checked. Trailing data after the
last declared block is left unread.

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

Release instructions are in [RELEASING.md](RELEASING.md).

## License

Licensed under **LGPL-2.0-or-later**; see [LICENSE](LICENSE).
Based on [Tilka's HVQM4 decoder](https://github.com/Tilka/hvqm4), via the
[mbcgh fork](https://github.com/mbcgh/h4m-video-decoder), retaining its
[original license](https://github.com/mbcgh/h4m-video-decoder/blob/0970075/README.md#license).
Upstream credits Tilka and flacs/hcs (container/audio).
Rust rewrite and synthetic tests by h4m contributors, 2026.
