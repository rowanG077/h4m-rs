# Original-movie validation for the 0.3.0 refactor

Validated 2026-09-17 after rebasing the refactor onto released `v0.2.0`
(`0f3097b5da6887a6c04501430fb96601efbf6bec`), using the complete extracted movie
directories of both Tales of
Symphonia discs: **11 files, 26,930 frames, 9,622,840,320 YUV bytes, zero differences**.
Every file was decoded in full, including both distinct `op.h4m` files. Presentation
indices, plane lengths, unique complete presentation ranges, and header frame
counts also matched. Audio is skipped by this video decoder.

The reference was the unchanged pixel decoder from `mbcgh/h4m-video-decoder` at
`02d66526e62e346c3ef7f92eac49cc07356a2c62`, called by `nix/reference-planes.c`.
The adapter preserves its frame-buffer rotation and streams native Y/U/V planes
in decoding order; the test compares bytes directly rather than hashes of output.
The source hashes below identify the locally supplied inputs. No movie assets
are distributed with this test.

| Original movie | Dimensions | Frames | Input SHA-256 |
|---|---|---:|---|
| `disc1/files/MOV/as1.h4m` | 640×336 | 3,111 | `d136b19b00d3f9c9102ba8c85ad0bc0bf278d970f3eec342dfac797dfd972de5` |
| `disc1/files/MOV/op.h4m` | 640×336 | 3,630 | `ae54c9c117bf126d03166a677b830464e7520c6a5181667970559ae43cb8e036` |
| `disc1/files/MOV/s01.h4m` | 640×480 | 1,918 | `8bf5e7227e634a61c76abb24bb5053e329508149ccf78fcff05681d45d49fd40` |
| `disc1/files/MOV/s03.h4m` | 640×480 | 689 | `c3c4c462d8ab7efd7ddaec7cbfd28a480bbbb3127079e90e792183a9a565742f` |
| `disc1/files/MOV/s07.h4m` | 640×480 | 629 | `1a58d2c55bc052ce91a0074e76bfced9ca515e8159729341396624a9ddac6319` |
| `disc1/files/MOV/s08.h4m` | 640×480 | 1,169 | `1a51353a2923acfe5078535878fbf9d86082be82d0549afa974df1e503ac47ac` |
| `disc2/files/MOV/as2.h4m` | 640×336 | 1,688 | `1cd7ecdde350503fdcff11a4114ca494369444190722cf3697d706c1ade6ff4e` |
| `disc2/files/MOV/as3.h4m` | 640×336 | 8,101 | `9319782aa91688980a4ad36efaa41a8a5bb94792989c3d730fd8a2835dbae8a1` |
| `disc2/files/MOV/op.h4m` | 640×336 | 3,627 | `53e52bc09a25e767b67ca301a43b0b29c829a1e120a02036d22619ddc33c372c` |
| `disc2/files/MOV/s09.h4m` | 640×480 | 1,379 | `cbd8ed83a65a840f9a9d2477a105c6ffdfcf5f3e85750b2c8b515b14adfd3ee3` |
| `disc2/files/MOV/s10.h4m` | 640×480 | 989 | `d9409a0d30143da2d87615b9e995f38141e5ef88b2f6ce56b46510e5cc87e97f` |

Reproduce with the complete extraction root, not an individual movie directory:

```sh
H4M_ORIGINALS=/path/to/extracted/discs nix develop -c ./scripts/test-originals.sh
```

Additional checks passed: Rust unit/integration/doc tests and strict Clippy with
default features, no features, and `alloc` only; 916 synthetic frames against the
C reference in both debug and release; rustfmt; rustdoc with warnings denied for
all three feature configurations; workflow linting; package verification; and
the native aarch64-linux Nix package, portability, and reference checks.
The synthetic corpus covers current-reference P pictures for both HVQM versions
and every supported sampling layout, including overlapping integer and
horizontal/vertical/diagonal fractional motion and transformed/literal blocks.

An external counting-allocator instrument also decoded all 26,930 frames through
each interface on the same refactored source:

- `SliceDecoder`: zero allocations during construction, decoding, and
  `Frame::to_rgb_into`. Input, frame buffers, block workspace, and RGB output were
  supplied before counting began.
- Standard I/O `Decoder`: five allocations during construction (three frames,
  one block workspace, one packet buffer), then zero during decoding. The caller's
  `BufReader` was created before counting began.

The instrument lives outside the crate; no unsafe allocator implementation or
new dependency was added to the decoder. The permanent buffer tests also run
without either `std` or `alloc` enabled, including stack-only frame storage,
undersized buffers, truncation, packet limits, and poisoned-decoder behavior.

P-picture fixes are already released in `0.2.0`. The breaking buffer/API refactor
remains under `Unreleased`, targeting `0.3.0`; the release workflow updates the
package version when that release is requested.
