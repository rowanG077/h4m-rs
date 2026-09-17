//! Opt-in, bounded-memory differential comparison of a local original-movie corpus.

use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

struct Reference(Child);

impl Drop for Reference {
    fn drop(&mut self) {
        // Also reap the reference if a decode error or mismatch panics the test.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn collect_movies(root: &Path, movies: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            collect_movies(&entry.path(), movies);
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("h4m"))
        {
            movies.push(entry.path());
        }
    }
}

#[test]
#[ignore = "set H4M_ORIGINALS and run scripts/test-originals.sh inside nix develop"]
fn every_original_movie_matches_reference() {
    let root = PathBuf::from(std::env::var_os("H4M_ORIGINALS").expect("set H4M_ORIGINALS"));
    let binary =
        PathBuf::from(std::env::var_os("H4M_REFERENCE_PLANES").expect("set H4M_REFERENCE_PLANES"));
    let mut movies = Vec::new();
    collect_movies(&root, &mut movies);
    movies.sort();
    assert!(!movies.is_empty(), "no H4M movies under {}", root.display());
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;
    for movie in &movies {
        let name = movie.strip_prefix(&root).unwrap().display().to_string();
        eprintln!("Comparing {name}");
        let mut decoder = h4m::Decoder::new(BufReader::new(File::open(movie).unwrap())).unwrap();
        let declared = decoder.header().video_frames;
        let mut reference = Reference(
            Command::new(&binary)
                .arg(movie)
                .arg("--stream")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("start reference plane adapter"),
        );
        let mut expected = BufReader::new(reference.0.stdout.take().unwrap());
        let mut pixels = Vec::new();
        let mut frames = 0;
        let mut bytes = 0u64;
        let mut seen = vec![false; declared as usize];
        while let Some(frame) = decoder
            .next_frame()
            .unwrap_or_else(|error| panic!("{name} after {frames} frames: {error}"))
        {
            let mut metadata = [0; 8];
            expected.read_exact(&mut metadata).unwrap_or_else(|error| {
                panic!(
                    "{name} frame {}: reference header: {error}",
                    frame.display_index
                )
            });
            let index = u32::from_be_bytes(metadata[..4].try_into().unwrap());
            let length = u32::from_be_bytes(metadata[4..].try_into().unwrap()) as usize;
            assert_eq!(index, frame.display_index, "{name}: presentation index");
            let actual_length = frame.y.data.len() + frame.u.data.len() + frame.v.data.len();
            assert_eq!(length, actual_length, "{name} frame {index}: plane length");
            assert!(index < declared, "{name}: out-of-range frame {index}");
            assert!(!seen[index as usize], "{name}: duplicate frame {index}");
            seen[index as usize] = true;
            pixels.resize(length, 0);
            expected
                .read_exact(&mut pixels)
                .unwrap_or_else(|error| panic!("{name} frame {index}: reference planes: {error}"));
            let mut offset = 0;
            for (plane, actual) in [
                ('Y', frame.y.data),
                ('U', frame.u.data),
                ('V', frame.v.data),
            ] {
                let expected = &pixels[offset..offset + actual.len()];
                if let Some(first) = actual.iter().zip(expected).position(|(a, b)| a != b) {
                    panic!(
                        "{name} frame {index} {:?}: {plane} byte {first}, Rust {}, C {}",
                        frame.kind, actual[first], expected[first]
                    );
                }
                offset += actual.len();
            }
            frames += 1;
            bytes += length as u64;
        }
        assert_eq!(
            expected.read(&mut [0]).unwrap(),
            0,
            "{name}: extra reference data"
        );
        assert!(
            reference.0.wait().unwrap().success(),
            "{name}: reference failed"
        );
        assert_eq!(frames, declared, "{name}: frame count");
        assert!(seen.into_iter().all(|seen| seen), "{name}: missing frame");
        eprintln!("PASS {name}: {frames} frames, {bytes} YUV bytes match exactly");
        total_frames += u64::from(frames);
        total_bytes += bytes;
    }
    eprintln!(
        "Compared all {} movies: {total_frames} frames, {total_bytes} YUV bytes, zero differences.",
        movies.len()
    );
}
