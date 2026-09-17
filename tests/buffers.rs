//! Caller-owned storage and container behavior, also run with no crate features.
mod common;
use h4m::{
    BlockState, BufferKind, ChromaSampling, DecoderBuffers, Error, FrameType, Limits, SliceDecoder,
    Version, VideoDecoder, VideoInfo,
};

fn info() -> VideoInfo {
    VideoInfo::new(Version::V15, 16, 16, ChromaSampling::Yuv420).unwrap()
}

#[test]
fn every_fixture_decodes_with_borrowed_storage() {
    for fixture in common::fixtures() {
        let mut input = common::container(&fixture, 2, true);
        input.extend_from_slice(b"unread trailer");
        let requirements = fixture.info.buffer_requirements();
        let mut frames = [
            vec![0xaa; requirements.frame_bytes + 1],
            vec![0xbb; requirements.frame_bytes + 1],
            vec![0xcc; requirements.frame_bytes + 1],
        ];
        let mut blocks = vec![BlockState::EMPTY; requirements.block_states];
        let [a, b, c] = &mut frames;
        let mut decoder = SliceDecoder::with_buffers(
            &input,
            DecoderBuffers {
                frames: [a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()],
                blocks: blocks.as_mut_slice(),
            },
            Limits::default(),
        )
        .unwrap();
        #[cfg(feature = "std")]
        let mut owned = h4m::Decoder::new(input.as_slice()).unwrap();
        let mut indices = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            assert_eq!(frame.info, fixture.info);
            #[cfg(feature = "std")]
            {
                let expected = owned.next_frame().unwrap().unwrap();
                assert_eq!(frame.kind, expected.kind);
                assert_eq!(frame.display_index, expected.display_index);
                assert_eq!(frame.y.data, expected.y.data);
                assert_eq!(frame.u.data, expected.u.data);
                assert_eq!(frame.v.data, expected.v.data);
            }
            indices.push(frame.display_index);
        }
        assert!(decoder.next_frame().unwrap().is_none());
        indices.sort_unstable();
        assert_eq!(
            indices,
            (0..fixture.packets.len() as u32 * 2).collect::<Vec<_>>()
        );
        let (remaining, _) = decoder.into_inner();
        assert_eq!(remaining, b"unread trailer");
        for (frame, guard) in frames.iter().zip([0xaa, 0xbb, 0xcc]) {
            assert_eq!(frame[requirements.frame_bytes], guard);
        }
    }
}

#[test]
fn stack_buffers_and_rgb_output_are_exact() {
    let info = info();
    assert_eq!(info.buffer_requirements().frame_bytes, 384);
    assert_eq!(info.buffer_requirements().block_states, 68);
    let mut a = [0; 384];
    let mut b = [0; 384];
    let mut c = [0; 384];
    let mut blocks = [BlockState::EMPTY; 68];
    let mut decoder = VideoDecoder::with_buffers(
        info,
        DecoderBuffers {
            frames: [&mut a[..], &mut b[..], &mut c[..]],
            blocks: &mut blocks[..],
        },
        Limits::default(),
    )
    .unwrap();
    let packet = common::intra(info, 2, 11, 0);
    let frame = decoder.decode(FrameType::I, 0, &packet).unwrap();
    assert_eq!(frame.y.data[0], 151);
    assert_eq!(frame.u.data[0], 222);
    assert_eq!(frame.v.data[0], 37);
    let mut short = [0xa5; 767];
    assert!(matches!(
        frame.to_rgb_into(&mut short),
        Err(Error::BufferTooSmall {
            buffer: BufferKind::Rgb,
            required: 768,
            provided: 767
        })
    ));
    assert!(short.iter().all(|&value| value == 0xa5));
    let mut rgb = [0xa5; 769];
    frame.to_rgb_into(&mut rgb).unwrap();
    assert_eq!(&rgb[..3], &[23, 183, 255]);
    assert_eq!(rgb[768], 0xa5);
}

#[test]
fn inline_arrays_stay_in_place_when_references_rotate() {
    let info = info();
    let mut decoder = VideoDecoder::with_buffers(
        info,
        DecoderBuffers {
            frames: [[0xaa; 385], [0xbb; 385], [0xcc; 385]],
            blocks: [BlockState::EMPTY; 68],
        },
        Limits::default(),
    )
    .unwrap();
    let mut addresses = Vec::new();
    for kind in [
        FrameType::I,
        FrameType::P,
        FrameType::P,
        FrameType::B,
        FrameType::B,
        FrameType::P,
    ] {
        let packet = if kind == FrameType::I {
            common::intra(info, 2, 11, 0)
        } else {
            common::inter(info, kind == FrameType::B, 0, 0, 0)
        };
        let frame = decoder.decode(kind, 0, &packet).unwrap();
        assert_eq!(
            [frame.y.data[0], frame.u.data[0], frame.v.data[0]],
            [151, 222, 37]
        );
        addresses.push(frame.y.data.as_ptr());
    }
    // Each reference picture occupies a different buffer. B pictures reuse
    // the spare buffer without rotating the reference pictures.
    assert_ne!(addresses[0], addresses[1]);
    assert_ne!(addresses[0], addresses[2]);
    assert_ne!(addresses[1], addresses[2]);
    assert!(addresses[3..]
        .iter()
        .all(|&address| address == addresses[0]));
    let buffers = decoder.into_buffers();
    for (frame, guard) in buffers.frames.iter().zip([0xaa, 0xbb, 0xcc]) {
        assert_eq!(frame[384], guard);
    }
}

#[test]
fn small_storage_is_rejected_before_any_buffer_is_cleared() {
    for short_frame in 0..3 {
        let mut frames = [[0xa5; 384]; 3];
        let mut blocks = [BlockState::EMPTY; 68];
        let [a, b, c] = &mut frames;
        let buffers = [a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()];
        let mut index = 0;
        let buffers = buffers.map(|frame| {
            let size = if index == short_frame { 383 } else { 384 };
            index += 1;
            &mut frame[..size]
        });
        assert!(matches!(
            VideoDecoder::with_buffers(
                info(),
                DecoderBuffers {
                    frames: buffers,
                    blocks: &mut blocks[..]
                },
                Limits::default()
            ),
            Err(Error::BufferTooSmall {
                buffer: BufferKind::Frame,
                ..
            })
        ));
        assert!(frames.iter().flatten().all(|&value| value == 0xa5));
    }
    let mut frames = [[0xa5; 384]; 3];
    let mut blocks = [BlockState::EMPTY; 67];
    let [a, b, c] = &mut frames;
    assert!(matches!(
        VideoDecoder::with_buffers(
            info(),
            DecoderBuffers {
                frames: [&mut a[..], &mut b[..], &mut c[..]],
                blocks: &mut blocks[..]
            },
            Limits::default()
        ),
        Err(Error::BufferTooSmall {
            buffer: BufferKind::BlockStates,
            required: 68,
            provided: 67
        })
    ));
    assert!(frames.iter().flatten().all(|&value| value == 0xa5));
}

#[test]
fn truncated_slice_input_and_invalid_packet_limits_are_sticky_errors() {
    let fixture = common::fixtures().remove(0);
    let input = common::container(&fixture, 1, true);
    for end in 68..input.len() {
        let mut frames = [[0; 384]; 3];
        let mut blocks = [BlockState::EMPTY; 68];
        let [a, b, c] = &mut frames;
        let mut decoder = SliceDecoder::with_buffers(
            &input[..end],
            DecoderBuffers {
                frames: [&mut a[..], &mut b[..], &mut c[..]],
                blocks: &mut blocks[..],
            },
            Limits::default(),
        )
        .unwrap();
        loop {
            match decoder.next_frame() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted truncation at {end}"),
                Err(_) => break,
            }
        }
        assert!(matches!(decoder.next_frame(), Err(Error::Failed)));
    }
    let mut input = input;
    input[40..44].copy_from_slice(&1u32.to_be_bytes());
    let mut frames = [[0; 384]; 3];
    let mut blocks = [BlockState::EMPTY; 68];
    let [a, b, c] = &mut frames;
    let mut decoder = SliceDecoder::with_buffers(
        &input,
        DecoderBuffers {
            frames: [&mut a[..], &mut b[..], &mut c[..]],
            blocks: &mut blocks[..],
        },
        Limits::default(),
    )
    .unwrap();
    assert!(matches!(
        decoder.next_frame(),
        Err(Error::Invalid("packet exceeds declared maximum"))
    ));
    assert!(matches!(decoder.next_frame(), Err(Error::Failed)));
}

#[test]
fn format_and_wire_codes_are_validated_at_the_boundary() {
    for (width, height) in [(0, 16), (16, 0), (15, 16), (16, 15)] {
        assert!(VideoInfo::new(Version::V15, width, height, ChromaSampling::Yuv420).is_err());
    }
    for sampling in [(0, 0), (1, 2), (3, 1)] {
        assert!(ChromaSampling::try_from(sampling).is_err());
    }
    for (code, expected) in [
        (0x10, FrameType::I),
        (0x20, FrameType::P),
        (0x30, FrameType::B),
    ] {
        assert_eq!(FrameType::try_from(code).unwrap(), expected);
        assert_eq!(expected as u16, code);
    }
    assert!(FrameType::try_from(0x40).is_err());
}
