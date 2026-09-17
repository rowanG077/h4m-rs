//! Dependency-free throughput benchmark. Run with `cargo bench --bench decode`.

#[path = "../tests/common/mod.rs"]
mod common;

use h4m::{FrameType, Version, VideoInfo, VideoPacketDecoder};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

fn main() {
    // Cargo also executes harness-free benches during `test --all-targets`.
    if !std::env::args().any(|arg| arg == "--bench") {
        return;
    }
    let info = VideoInfo::new(
        Version::V15,
        640,
        480,
        h4m::ChromaSampling::try_from((2, 2)).unwrap(),
    )
    .unwrap();
    for (label, intra_mode, inter_mode, rgb) in [
        ("I literal", 2, None, false),
        ("I mixed AOT", 3, None, false),
        ("P copy", 3, Some(0), false),
        ("P mixed AOT", 3, Some(2), false),
        ("I mixed AOT + RGB", 3, None, true),
    ] {
        let intra = common::intra(info, intra_mode, 13, 0);
        let mut decoder = VideoPacketDecoder::new(info).unwrap();
        decoder.decode(FrameType::I, 0, &intra).unwrap();
        let packet = inter_mode
            .map(|m| common::inter(info, false, m, 19, 0))
            .unwrap_or(intra);
        let kind = if inter_mode.is_some() {
            FrameType::P
        } else {
            FrameType::I
        };
        let mut scratch = Vec::new();
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_secs(1) {
            let frame = decoder.decode(kind, 0, black_box(&packet)).unwrap();
            if rgb {
                frame.to_rgb(&mut scratch).unwrap();
                black_box(&scratch);
            }
            black_box(frame.y().data());
            frames += 1;
        }
        let fps = frames as f64 / start.elapsed().as_secs_f64();
        println!(
            "{label:20} {fps:9.1} frames/s  {:8.1} megapixels/s  {:8.1} us/frame",
            fps * f64::from(info.width()) * f64::from(info.height()) / 1e6,
            1e6 / fps
        );
    }
    if let Some(path) = std::env::var_os("H4M_BENCH_FILE") {
        let data = std::fs::read(path).expect("read H4M_BENCH_FILE");
        let mut decoder = h4m::VideoDecoder::new(data.as_slice()).unwrap();
        let start = Instant::now();
        let mut frames = 0;
        while let Some(frame) = decoder.next_frame().unwrap() {
            black_box(frame.y().data());
            frames += 1;
        }
        println!(
            "Real file: {frames} frames in {:?} ({:.1} frames/s)",
            start.elapsed(),
            frames as f64 / start.elapsed().as_secs_f64()
        );
    }
}
