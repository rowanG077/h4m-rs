//! CLI extraction, presentation naming, and overwrite protection.

mod common;

use std::{fs, process::Command};

#[test]
fn extract_ppm_and_yuv() {
    let binary = env!("CARGO_BIN_EXE_h4m");
    let root = std::env::temp_dir().join(format!("h4m-cli-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = common::fixtures().remove(7);
    let data = common::container(&fixture, 1, true);
    let input = root.join("input.h4m");
    fs::write(&input, &data).unwrap();
    for yuv in [false, true] {
        let output = root.join(if yuv { "planes" } else { "frames" });
        let mut command = Command::new(binary);
        command.arg(&input).arg(&output);
        if yuv {
            command.arg("--yuv");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_dir(&output).unwrap().count(),
            fixture.packets.len()
        );
        let mut decoder = h4m::Decoder::new(&data[..]).unwrap();
        let mut rgb = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            let mut expected = Vec::new();
            if yuv {
                frame.write_yuv(&mut expected).unwrap();
            } else {
                frame.write_ppm(&mut expected, &mut rgb).unwrap();
            }
            let extension = if yuv { "yuv" } else { "ppm" };
            let actual =
                fs::read(output.join(format!("frame_{:010}.{extension}", frame.display_index)))
                    .unwrap();
            assert_eq!(actual, expected);
        }
        let result = command.output().unwrap();
        assert!(
            !result.status.success(),
            "existing output must not be overwritten"
        );
    }
    assert!(Command::new(binary)
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new(binary)
        .arg("--version")
        .output()
        .unwrap()
        .status
        .success());
    fs::remove_dir_all(root).unwrap();
}
