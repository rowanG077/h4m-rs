//! Extract one explicitly selected track as interleaved little-endian PCM16.

use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, BufWriter, Write},
    process::ExitCode,
};

const USAGE: &str = "usage: audio INPUT.h4m TRACK OUTPUT.pcm";

fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let input = args.next().ok_or(USAGE)?;
    if input == "--help" || input == "-h" {
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        println!("{USAGE}\nTRACK is one-based. Existing output is never overwritten.");
        return Ok(());
    }
    let track = args.next().ok_or(USAGE)?;
    let output = args.next().ok_or(USAGE)?;
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    let track: u16 = track
        .to_str()
        .ok_or("track must be a one-based integer")?
        .parse()?;
    let mut decoder = h4m::AudioDecoder::new(BufReader::new(File::open(input)?), track)?;
    let mut writer = BufWriter::new(File::options().write(true).create_new(true).open(output)?);
    while let Some(pcm) = decoder.next_block()? {
        for sample in pcm {
            writer.write_all(&sample.to_le_bytes())?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("audio: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "../tests/common/audio.rs"]
mod fixtures;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_errors_are_recoverable() {
        for args in [
            vec![],
            vec!["input"],
            vec!["input", "1"],
            vec!["input", "1", "output", "extra"],
        ] {
            assert!(run(args.into_iter().map(OsString::from)).is_err());
        }
        assert!(run([OsString::from("--help")]).is_ok());
    }

    #[test]
    fn extracts_selected_track_and_never_overwrites_input_or_output() {
        let root = std::env::temp_dir().join(format!("h4m-audio-example-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("input.h4m");
        let output = root.join("output.pcm");
        let data = fixtures::fixture();
        std::fs::write(&input, &data).unwrap();
        assert!(run([
            input.clone().into_os_string(),
            "1".into(),
            input.clone().into_os_string()
        ])
        .is_err());
        assert_eq!(std::fs::read(&input).unwrap(), data);
        assert!(run([
            input.clone().into_os_string(),
            "0".into(),
            output.clone().into_os_string()
        ])
        .is_err());
        assert!(!output.exists());
        let args = [
            input.into_os_string(),
            "2".into(),
            output.clone().into_os_string(),
        ];
        run(args.clone()).unwrap();
        let expected: Vec<_> = [20i16, 10, 31, 21, 1, -9, 5, -5, 8, -2]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        assert!(run(args).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        std::fs::remove_dir_all(root).unwrap();
    }
}
