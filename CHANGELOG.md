# Changelog

## Unreleased

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
