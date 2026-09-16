//! H4M frame extraction command.

use std::{
    env,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::PathBuf,
    process::ExitCode,
};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let Some(input) = args.next() else {
        return Err("usage: h4m INPUT.h4m OUTPUT_DIRECTORY [--yuv]".into());
    };
    if input == "--help" || input == "-h" {
        println!("Usage: h4m INPUT.h4m OUTPUT_DIRECTORY [--yuv]\n\nExtract video frames as PPM (or planar YUV with --yuv).\nFilenames use presentation order. Audio is skipped.");
        return Ok(());
    }
    if input == "--version" {
        println!("h4m {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let output = PathBuf::from(args.next().ok_or("missing output directory")?);
    let yuv = match args.next() {
        None => false,
        Some(flag) if flag == "--yuv" => true,
        _ => return Err("expected --yuv".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let mut decoder = h4m::Decoder::new(BufReader::new(File::open(input)?))?;
    fs::create_dir_all(&output)?;
    let mut rgb = Vec::new();
    let mut count = 0u64;
    while let Some(frame) = decoder.next_frame()? {
        let extension = if yuv { "yuv" } else { "ppm" };
        let name = output.join(format!("frame_{:010}.{extension}", frame.display_index));
        // Refuse to overwrite existing output, including symlinks.
        let file = File::options().write(true).create_new(true).open(name)?;
        let mut writer = BufWriter::new(file);
        if yuv {
            frame.write_yuv(&mut writer)?;
        } else {
            frame.write_ppm(&mut writer, &mut rgb)?;
        }
        writer.flush()?;
        count += 1;
    }
    eprintln!("Decoded {count} video frames.");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("h4m: {error}");
            ExitCode::FAILURE
        }
    }
}
