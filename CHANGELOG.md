# Changelog

## Unreleased

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
