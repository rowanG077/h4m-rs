//! Decoder correctness and malformed-input regression tests.

mod common;

use h4m::{Decoder, Error, FrameType, Limits, Version, VideoDecoder, VideoInfo};

#[test]
fn synthetic_corpus_decodes() {
    for fixture in common::fixtures() {
        let data = common::container(&fixture, 2, true);
        let mut decoder = Decoder::new(&data[..]).unwrap();
        let mut seen = Vec::new();
        while let Some(frame) = decoder
            .next_frame()
            .unwrap_or_else(|e| panic!("{}: {e}", fixture.name))
        {
            assert_eq!(
                frame.y.data.len(),
                fixture.info.width as usize * fixture.info.height as usize
            );
            seen.push(frame.display_index);
        }
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..fixture.packets.len() as u32 * 2).collect::<Vec<_>>()
        );
        assert!(decoder.next_frame().unwrap().is_none());
    }
}

#[test]
fn literal_pixels_and_prediction_are_exact() {
    for (hs, vs) in [(1, 1), (2, 1), (2, 2)] {
        let info = VideoInfo {
            version: Version::V15,
            width: 32,
            height: 16,
            horizontal_sampling: hs,
            vertical_sampling: vs,
        };
        let mut decoder = VideoDecoder::new(info).unwrap();
        let packet = common::intra(info, 2, 11, 0);
        let frame = decoder.decode(FrameType::I, 0, &packet).unwrap();
        let mut original = Vec::new();
        frame.write_yuv(&mut original).unwrap();
        for (plane, p) in [frame.y, frame.u, frame.v].iter().enumerate() {
            for by in 0..p.height / 4 {
                for bx in 0..p.width / 4 {
                    for y in 0..4 {
                        for x in 0..4 {
                            let seed = by * (p.width / 4) + bx + 11;
                            let expected =
                                ((seed * 37 + (y * 4 + x) * 19 + plane * 71) % 256) as u8;
                            assert_eq!(p.data[(by * 4 + y) * p.width + bx * 4 + x], expected);
                        }
                    }
                }
            }
        }
        for kind in [FrameType::P, FrameType::B, FrameType::B, FrameType::P] {
            let packet = common::inter(info, kind == FrameType::B, 0, 0, 0);
            let frame = decoder.decode(kind, 1, &packet).unwrap();
            let mut copy = Vec::new();
            frame.write_yuv(&mut copy).unwrap();
            assert_eq!(copy, original);
        }
    }
}

#[test]
fn every_truncation_is_an_error() {
    let fixture = common::fixtures().remove(0);
    let data = common::container(&fixture, 1, true);
    for end in 0..data.len() {
        let result = (|| {
            let mut d = Decoder::new(&data[..end])?;
            while d.next_frame()?.is_some() {}
            Ok::<_, Error>(())
        })();
        assert!(
            result.is_err(),
            "accepted truncated file of {end}/{} bytes",
            data.len()
        );
    }
}

#[test]
fn limits_and_poisoning() {
    let fixture = common::fixtures().remove(0);
    let data = common::container(&fixture, 1, false);
    assert!(matches!(
        Decoder::with_limits(
            &data[..],
            Limits {
                max_pixels: 1,
                ..Limits::default()
            }
        ),
        Err(Error::Limit(_))
    ));
    let mut d = Decoder::with_limits(
        &data[..],
        Limits {
            max_frame_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(d.next_frame(), Err(Error::Limit(_))));
    assert!(matches!(d.next_frame(), Err(Error::Failed)));
    let mut d = VideoDecoder::new(fixture.info).unwrap();
    assert!(d.decode(FrameType::P, 0, &fixture.packets[0].data).is_err());
    assert!(matches!(
        d.decode(FrameType::I, 0, &fixture.packets[0].data),
        Err(Error::Failed)
    ));
}

#[test]
fn malformed_inputs_do_not_panic() {
    let fixture = common::fixtures().remove(3);
    let data = common::container(&fixture, 1, false);
    let mut state = 0x12345678u32;
    for _ in 0..2000 {
        let mut bytes = data.clone();
        for _ in 0..4 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let index = state as usize % bytes.len();
            bytes[index] ^= (state >> 24) as u8;
        }
        if let Ok(mut d) = Decoder::with_limits(
            &bytes[..],
            Limits {
                max_pixels: 65536,
                max_frame_bytes: 65536,
            },
        ) {
            while let Ok(Some(_)) = d.next_frame() {}
        }
    }
}

#[test]
fn inter_packet_mutations_do_not_panic() {
    let fixture = common::fixtures().remove(7);
    let mut state = 0xabcdef01u32;
    for _ in 0..2000 {
        let mut decoder = VideoDecoder::new(fixture.info).unwrap();
        decoder
            .decode(FrameType::I, 0, &fixture.packets[0].data)
            .unwrap();
        let mut bytes = fixture.packets[1].data.clone();
        for _ in 0..4 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let index = state as usize % bytes.len();
            bytes[index] ^= (state >> 24) as u8;
        }
        let _ = decoder.decode(FrameType::P, 1, &bytes);
    }
}

#[test]
fn invalid_headers_and_block_accounting_are_rejected() {
    let fixture = common::fixtures().remove(0);
    let original = common::container(&fixture, 1, true);
    for (offset, byte) in [
        (0, b'X'),
        (19, 67),
        (52, 255),
        (56, 3),
        (58, 1),
        (59, 1),
        (84, 0),
        (79, 2),
        (95, 255),
        (107, 0),
    ] {
        let mut bytes = original.clone();
        bytes[offset] = byte;
        let result = (|| {
            let mut d = Decoder::with_limits(
                &bytes[..],
                Limits {
                    max_pixels: 65536,
                    max_frame_bytes: 65536,
                },
            )?;
            while d.next_frame()?.is_some() {}
            Ok::<(), Error>(())
        })();
        assert!(result.is_err(), "invalid byte at {offset} was accepted");
    }
}

#[test]
fn p_picture_second_reference_uses_the_current_buffer() {
    let info = VideoInfo {
        version: Version::V15,
        width: 32,
        height: 16,
        horizontal_sampling: 2,
        vertical_sampling: 2,
    };
    let mut decoder = VideoDecoder::new(info).unwrap();
    let mut expected = Vec::new();
    decoder
        .decode(FrameType::I, 0, &common::intra(info, 2, 11, 0))
        .unwrap()
        .write_yuv(&mut expected)
        .unwrap();
    for (index, seed) in [(1, 19), (2, 31)] {
        decoder
            .decode(FrameType::I, index, &common::intra(info, 2, seed, 0))
            .unwrap();
    }
    // The rotating destination now holds I0; the other two references hold
    // I1 and I2. Copying the current buffer must preserve I0 exactly.
    let mut actual = Vec::new();
    decoder
        .decode(FrameType::P, 3, &common::inter(info, false, 4, 0, 0))
        .unwrap()
        .write_yuv(&mut actual)
        .unwrap();
    assert_eq!(actual, expected);
}
