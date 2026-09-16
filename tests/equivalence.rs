//! Opt-in differential comparison against the pinned, original C decoder.

mod common;

use std::{fs, path::PathBuf, process::Command};

#[test]
#[ignore = "run scripts/test-reference.sh inside nix develop"]
fn original_decoder_equivalence() {
    let binary = PathBuf::from(std::env::var_os("H4M_REFERENCE").expect("set H4M_REFERENCE"));
    let binary = fs::canonicalize(binary).unwrap();
    let planes_binary =
        PathBuf::from(std::env::var_os("H4M_REFERENCE_PLANES").expect("set H4M_REFERENCE_PLANES"));
    let planes_binary = fs::canonicalize(planes_binary).unwrap();
    let root = std::env::temp_dir().join(format!("h4m-equivalence-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut frames = 0;
    for fixture in common::fixtures() {
        let dir = root.join(&fixture.name);
        fs::create_dir_all(&dir).unwrap();
        let input = common::container(&fixture, 2, true);
        fs::write(dir.join("input.h4m"), &input).unwrap();
        let output = Command::new(&binary)
            .current_dir(&dir)
            .args(["input.h4m", "audio.wav"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: C decoder failed: {}\n{}",
            fixture.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&planes_binary)
            .current_dir(&dir)
            .arg("input.h4m")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: plane adapter failed: {}",
            fixture.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let mut decoder = h4m::Decoder::new(&input[..]).unwrap();
        let mut rgb = Vec::new();
        while let Some(frame) = decoder
            .next_frame()
            .unwrap_or_else(|e| panic!("{}: {e}", fixture.name))
        {
            let expected =
                fs::read(dir.join(format!("output/video_rgb_{:04}.ppm", frame.display_index)))
                    .unwrap();
            let mut actual = Vec::new();
            frame.write_ppm(&mut actual, &mut rgb).unwrap();
            if fixture.info.horizontal_sampling == 2
                && fixture.info.vertical_sampling == 2
                && actual != expected
            {
                let first = actual.iter().zip(&expected).position(|(a, b)| a != b);
                panic!(
                    "{} frame {} {:?}: mismatch at byte {first:?}, Rust {:?}, C {:?}",
                    fixture.name,
                    frame.display_index,
                    frame.kind,
                    first.map(|n| actual[n]),
                    first.map(|n| expected[n])
                );
            }
            let expected_yuv =
                fs::read(dir.join(format!("output/video_yuv_{:04}.yuv", frame.display_index)))
                    .unwrap();
            let mut actual_yuv = Vec::new();
            frame.write_yuv(&mut actual_yuv).unwrap();
            assert_eq!(
                actual_yuv, expected_yuv,
                "{} frame {}: YUV mismatch",
                fixture.name, frame.display_index
            );
            frames += 1;
        }
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
    eprintln!("Compared {frames} synthetic frames byte-for-byte with the original decoder.");
}
