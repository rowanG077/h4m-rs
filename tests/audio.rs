//! Synthetic IMA packets exercise initialization, continuation, and track isolation.

use h4m::{
    AudioBufferRequirements, AudioInfo, AudioLimits, AudioPacketDecoder, BufferKind, Error,
    SliceAudioDecoder,
};
#[path = "common/audio.rs"]
mod fixtures;
use fixtures::{fixture, packet};

fn decode(b: &[u8], track: u16) -> Result<Vec<i16>, Error> {
    let mut d =
        SliceAudioDecoder::with_buffer_and_limits(b, track, [0i16; 32], AudioLimits::default())?;
    let mut out = Vec::new();
    while let Some(p) = d.next_block()? {
        out.extend_from_slice(p);
    }
    Ok(out)
}

#[test]
fn initialization_continuation_and_track_isolation() {
    let b = fixture();
    assert_eq!(
        decode(&b, 1).unwrap(),
        [200, 100, 201, 101, 200, 100, 201, 101, 202, 102]
    );
    assert_eq!(
        decode(&b, 2).unwrap(),
        [20, 10, 31, 21, 1, -9, 5, -5, 8, -2]
    );
    let mut other = b.clone();
    other[115] ^= 0x55;
    assert_eq!(decode(&other, 1).unwrap(), decode(&b, 1).unwrap());
    let d =
        SliceAudioDecoder::with_buffer_and_limits(b.as_slice(), 2, [0; 32], AudioLimits::default())
            .unwrap();
    assert_eq!(d.metadata().track(), 2);
    assert_eq!(d.metadata().sample_rate(), 32028);
}

#[test]
fn truncation_bounds_profiles_and_poisoning() {
    let b = fixture();
    for n in 0..b.len() {
        assert!(decode(&b[..n], 1).is_err(), "prefix {n}");
    }
    for track in [0, 3, u16::MAX] {
        assert!(SliceAudioDecoder::with_buffer_and_limits(
            b.as_slice(),
            track,
            [0; 32],
            AudioLimits::default()
        )
        .is_err());
    }
    assert!(SliceAudioDecoder::with_buffer_and_limits(
        b.as_slice(),
        1,
        [0; 32],
        AudioLimits {
            max_packet_bytes: 21,
            ..Default::default()
        }
    )
    .is_err());
    for (at, v) in [(62, 1), (91, 2), (102, 89), (95, 255), (84, 0)] {
        let mut bad = b.clone();
        bad[at] = v;
        assert!(decode(&bad, 1).is_err(), "offset {at}");
    }
    let mut bad = b;
    bad[91] = 2;
    let mut d = SliceAudioDecoder::with_buffer_and_limits(
        bad.as_slice(),
        1,
        [0; 32],
        AudioLimits::default(),
    )
    .unwrap();
    assert!(d.next_block().is_err());
    assert!(matches!(d.next_block(), Err(Error::Failed)));
}

#[test]
fn mutation_fuzz_is_bounded_and_never_panics() {
    let mut rng = 0x456789abu32;
    for _ in 0..10000 {
        let mut b = fixture();
        for _ in 0..4 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let i = rng as usize % b.len();
            b[i] ^= (rng >> 24) as u8;
        }
        if let Ok(mut d) = SliceAudioDecoder::with_buffer_and_limits(
            b.as_slice(),
            1,
            [0; 4096],
            AudioLimits {
                max_packet_bytes: 4096,
                max_frames_per_packet: 2048,
            },
        ) {
            for _ in 0..8 {
                match d.next_block() {
                    Ok(Some(_)) => {}
                    _ => break,
                }
            }
        }
    }
}

#[test]
fn predictor_saturates_at_both_pcm16_limits() {
    let mut b = fixture();
    b[100..103].copy_from_slice(&[0x80, 8, 88]); // R = -32760.
    b[103..106].copy_from_slice(&[0x7f, 0xf8, 88]); // L = 32760.
    b[106..108].copy_from_slice(&[0xf7, 0x80]);
    assert_eq!(
        &decode(&b, 1).unwrap()[..6],
        &[32760, -32760, 32767, -32768, 32767, -32768]
    );
}

#[test]
fn packet_decoder_uses_caller_storage_and_errors_are_transactional() {
    let b = fixture();
    let mut d = AudioPacketDecoder::new(AudioInfo::parse(&b, 1).unwrap()).unwrap();
    assert_eq!(d.metadata().tracks(), 2);
    assert_eq!(d.buffer_requirements().pcm_samples, 18);
    let first = &b[96..118];
    let second = &b[126..134];
    let mut output = [1234; 8];
    let error = d
        .decode_into(h4m::AudioPacketMode::Initialize, first, &mut output[..5])
        .unwrap_err();
    assert!(matches!(
        error,
        Error::BufferTooSmall {
            buffer: BufferKind::AudioPcm,
            required: 6,
            provided: 5,
        }
    ));
    assert_eq!(output, [1234; 8]);
    // A rejected initialization must not enable continuation.
    assert!(d
        .decode_into(h4m::AudioPacketMode::Continue, second, &mut output)
        .is_err());
    assert_eq!(
        d.decode_into(h4m::AudioPacketMode::Initialize, first, &mut output)
            .unwrap(),
        [200, 100, 201, 101, 200, 100]
    );
    assert_eq!(&output[6..], &[1234; 2]);
    let previous = output;
    // Reject every truncation and invalid mode without touching established state.
    for len in 0..first.len() {
        assert!(d
            .decode_into(h4m::AudioPacketMode::Initialize, &first[..len], &mut output)
            .is_err());
        assert_eq!(output, previous);
    }
    for mode in [0, 1, 4, u16::MAX] {
        assert!(h4m::AudioPacketMode::try_from(mode).is_err());
    }
    // The second channel's bad index must not partially reset the first channel.
    let mut bad = first.to_vec();
    bad[4..7].copy_from_slice(&[0x12, 0x34, 89]);
    bad[7..10].copy_from_slice(&[0x23, 0x45, 40]);
    assert!(d
        .decode_into(h4m::AudioPacketMode::Initialize, &bad, &mut output)
        .is_err());
    assert!(d
        .decode_into(h4m::AudioPacketMode::Continue, second, &mut output[..3])
        .is_err());
    assert_eq!(output, previous);
    assert_eq!(
        d.decode_into(h4m::AudioPacketMode::Continue, second, &mut output)
            .unwrap(),
        [201, 101, 202, 102]
    );
    assert_eq!(&output[4..], &previous[4..]);
}

#[test]
fn packet_decoder_reset_limits_and_minimum_initialization() {
    let b = fixture();
    let mut d = AudioPacketDecoder::new(AudioInfo::parse(&b, 2).unwrap()).unwrap();
    let mut output = [0; 32];
    for _ in 0..2 {
        assert_eq!(
            d.decode_into(h4m::AudioPacketMode::Initialize, &b[96..118], &mut output)
                .unwrap(),
            [20, 10, 31, 21, 1, -9]
        );
        assert_eq!(
            d.decode_into(h4m::AudioPacketMode::Continue, &b[126..134], &mut output)
                .unwrap(),
            [5, -5, 8, -2]
        );
    }
    // One sample uses only predictors; the final encoded byte is padding.
    let p = packet(
        3,
        1,
        &[&[0, 100, 0, 0, 200, 0, 0xff], &[0, 10, 0, 0, 20, 0, 0xff]],
    );
    assert_eq!(
        d.decode_into(h4m::AudioPacketMode::Initialize, &p[8..], &mut output)
            .unwrap(),
        [20, 10]
    );
    for samples in [0, 10, u32::MAX] {
        let p = packet(2, samples, &[&[0], &[0]]);
        assert!(matches!(
            d.decode_into(h4m::AudioPacketMode::Continue, &p[8..], &mut output),
            Err(Error::Limit(_))
        ));
    }
    assert!(matches!(
        d.decode_into(h4m::AudioPacketMode::Initialize, &[0; 23], &mut output),
        Err(Error::Limit(_))
    ));
    // A policy below the declared bound rejects setup; it cannot shrink sizing.
    let info = AudioInfo::parse(&b, 1).unwrap();
    let limits = AudioLimits {
        max_frames_per_packet: 2,
        ..Default::default()
    };
    assert!(matches!(
        AudioPacketDecoder::with_limits(info, limits),
        Err(Error::Limit(_))
    ));
    assert_eq!(info.buffer_requirements().pcm_samples, 18);
}

#[test]
fn slice_constructor_rejects_small_storage_before_touching_it() {
    let b = fixture();
    let limits = AudioLimits::default();
    let required = AudioInfo::parse(&b, 1)
        .unwrap()
        .buffer_requirements()
        .pcm_samples;
    assert_eq!(required, 18);
    // Includes a buffer large enough for the first packet but below the bound.
    for provided in [0, 5, 6, required - 1] {
        let mut output = [5678; 18];
        assert!(matches!(
            SliceAudioDecoder::with_buffer_and_limits(&b, 1, &mut output[..provided], limits),
            Err(Error::BufferTooSmall { buffer: BufferKind::AudioPcm, required: 18, provided: n }) if n == provided
        ));
        assert_eq!(output, [5678; 18]);
        // The caller retains the original input/storage and can retry construction.
        let mut decoder =
            SliceAudioDecoder::with_buffer_and_limits(&b, 1, &mut output[..], limits).unwrap();
        assert_eq!(decoder.buffer_requirements().pcm_samples, required);
        assert_eq!(
            decoder.next_block().unwrap().unwrap(),
            [200, 100, 201, 101, 200, 100]
        );
        assert_eq!(decoder.next_block().unwrap().unwrap(), [201, 101, 202, 102]);
        assert!(decoder.next_block().unwrap().is_none());
        let (rest, storage) = decoder.into_inner();
        assert!(rest.is_empty());
        assert_eq!(&storage[6..], [5678; 12]);
    }
}

#[test]
fn metadata_reports_storage_without_constructing_a_decoder() {
    let b = fixture();
    let info = AudioInfo::parse(&b[..68], 2).unwrap();
    assert_eq!(info, AudioInfo::parse(&b, 2).unwrap());
    assert_eq!(info.sample_rate(), 32028);
    assert_eq!(info.channels(), 2);
    assert_eq!(info.tracks(), 2);
    assert_eq!(info.track(), 2);
    assert_eq!(info.packets(), 2);
    let limits = AudioLimits::default();
    let required = info.buffer_requirements();
    assert_eq!(
        required,
        AudioBufferRequirements {
            packet_bytes: 22,
            pcm_samples: 18
        }
    );
    let mut pcm = vec![1234; required.pcm_samples];
    let mut d = SliceAudioDecoder::with_buffer_and_limits(&b, 2, &mut pcm[..], limits).unwrap();
    assert_eq!(d.metadata(), info);
    assert_eq!(d.buffer_requirements(), required);
    assert_eq!(d.next_block().unwrap().unwrap(), [20, 10, 31, 21, 1, -9]);
    assert_eq!(d.next_block().unwrap().unwrap(), [5, -5, 8, -2]);
    assert!(d.next_block().unwrap().is_none());
    let (rest, output) = d.into_inner();
    assert!(rest.is_empty());
    assert_eq!(&output[6..], [1234; 12]);
    assert_eq!(
        AudioPacketDecoder::with_limits(info, limits)
            .unwrap()
            .buffer_requirements(),
        required
    );
    for len in 0..68 {
        assert!(matches!(
            AudioInfo::parse(&b[..len], 1),
            Err(Error::Truncated)
        ));
    }
    for track in [0, 3, u16::MAX] {
        assert!(AudioInfo::parse(&b, track).is_err());
    }
}

#[test]
fn header_and_limit_failures_are_reported_during_setup() {
    let b = fixture();
    let info = AudioInfo::parse(&b, 1).unwrap();
    for limits in [
        AudioLimits {
            max_packet_bytes: 0,
            max_frames_per_packet: 100,
        },
        AudioLimits {
            max_packet_bytes: 21,
            max_frames_per_packet: 100,
        },
        AudioLimits {
            max_packet_bytes: 22,
            max_frames_per_packet: 0,
        },
    ] {
        assert!(matches!(info.validate_limits(limits), Err(Error::Limit(_))));
        assert!(matches!(
            AudioPacketDecoder::with_limits(info, limits),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            SliceAudioDecoder::with_buffer_and_limits(&b, 1, [0; 32], limits),
            Err(Error::Limit(_))
        ));
    }
    // Two tracks need at least 18 bytes for even one initialized sample each.
    for maximum in [0u32, 4, 17] {
        let mut header = b[..68].to_vec();
        header[48..52].copy_from_slice(&maximum.to_be_bytes());
        assert!(matches!(
            AudioInfo::parse(&header, 1),
            Err(Error::Invalid(_))
        ));
    }
    // Querying sizes remains possible even when the default policy rejects them.
    // Unrepresentable sizes fail at parsing, not during the sizing query.
    let mut header = b[..68].to_vec();
    header[63] = 0;
    header[48..52].copy_from_slice(&u32::MAX.to_be_bytes());
    #[cfg(target_pointer_width = "64")]
    {
        let info = AudioInfo::parse(&header, 1).unwrap();
        let required = info.buffer_requirements();
        assert_eq!(required.packet_bytes, u32::MAX as usize);
        assert_eq!(required.pcm_samples, 2 * (u32::MAX as usize - 4));
        assert!(matches!(
            info.validate_limits(AudioLimits::default()),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            AudioPacketDecoder::new(info),
            Err(Error::Limit(_))
        ));
        let maximum = AudioLimits {
            max_packet_bytes: usize::MAX,
            max_frames_per_packet: usize::MAX,
        };
        assert!(info.validate_limits(maximum).is_ok());
        assert_eq!(
            AudioPacketDecoder::with_limits(info, maximum)
                .unwrap()
                .buffer_requirements(),
            required
        );
    }

    #[cfg(target_pointer_width = "32")]
    {
        assert!(matches!(
            AudioInfo::parse(&header, 1),
            Err(Error::Limit("audio packet bytes"))
        ));
        header[48..52].copy_from_slice(&(i32::MAX as u32).to_be_bytes());
        assert!(matches!(
            AudioInfo::parse(&header, 1),
            Err(Error::Limit("audio PCM size"))
        ));
    }
}

#[test]
fn reported_capacity_covers_track_counts_and_both_packet_modes() {
    // Stress units, maximum track ID, and limits smaller than the header bound.
    for tracks in [1u16, 2, 3, 256] {
        for frames in [1usize, 2, 6, 7, 8, 31] {
            let mut header = fixture()[..68].to_vec();
            header[63] = (tracks - 1) as u8;
            let initialization: Vec<Vec<u8>> = (0..tracks)
                .map(|t| {
                    let sample = (t as i16 + 100).to_be_bytes();
                    vec![sample[0], sample[1], 0, sample[0], sample[1], 0, 0xff]
                })
                .collect();
            let continuation = vec![0x11; frames];
            let first = packet(
                3,
                1,
                &initialization.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            );
            let second = packet(
                2,
                frames as u32,
                &vec![continuation.as_slice(); usize::from(tracks)],
            );
            let max = (first.len().max(second.len()) - 8) as u32;
            header[48..52].copy_from_slice(&max.to_be_bytes());
            let info = AudioInfo::parse(&header, tracks).unwrap();
            for cap in [1, frames, frames.max(7), frames.max(7) + 1, usize::MAX] {
                let limits = AudioLimits {
                    max_frames_per_packet: cap,
                    ..Default::default()
                };
                let required = info.buffer_requirements();
                assert_eq!(required.packet_bytes, max as usize);
                assert_eq!(required.pcm_samples, 2 * frames.max(7));
                if cap < frames.max(7) {
                    assert!(matches!(
                        AudioPacketDecoder::with_limits(info, limits),
                        Err(Error::Limit(_))
                    ));
                    assert_eq!(info.buffer_requirements(), required);
                    continue;
                }
                let mut pcm = vec![0; required.pcm_samples];
                let mut d = AudioPacketDecoder::with_limits(info, limits).unwrap();
                assert_eq!(
                    d.decode_into(h4m::AudioPacketMode::Initialize, &first[8..], &mut pcm)
                        .unwrap(),
                    [tracks as i16 + 99; 2]
                );
                let decoded = d
                    .decode_into(h4m::AudioPacketMode::Continue, &second[8..], &mut pcm)
                    .unwrap();
                assert_eq!(decoded.len(), frames * 2);
                assert_eq!(
                    decoded[decoded.len() - 1],
                    tracks as i16 + 99 + frames as i16
                );
            }
        }
    }
}

#[test]
fn container_skips_video_consumes_trailer_and_preserves_trailing_data() {
    let mut b = fixture();
    // Add a video packet between initialization and continuation.
    let video: [u8; 13] = [0, 1, 0, 0x10, 0, 0, 0, 5, 11, 22, 33, 44, 55];
    b.splice(118..118, video);
    for (at, increase) in [(20, 13 + 16), (28, 1), (72, 13), (76, 1)] {
        let old = u32::from_be_bytes(b[at..at + 4].try_into().unwrap());
        b[at..at + 4].copy_from_slice(&(old + increase).to_be_bytes());
    }
    b.extend([0xab; 16]);
    let end = b.len();
    b.extend([0x12, 0x34, 0x56]);
    let mut output = [999; 18];
    let mut d =
        SliceAudioDecoder::with_buffer_and_limits(&b, 1, &mut output[..], AudioLimits::default())
            .unwrap();
    assert_eq!(d.header().video_frames(), 1);
    assert_eq!(
        d.next_block().unwrap().unwrap(),
        [200, 100, 201, 101, 200, 100]
    );
    assert_eq!(d.next_block().unwrap().unwrap(), [201, 101, 202, 102]);
    assert!(d.next_block().unwrap().is_none());
    assert!(d.next_block().unwrap().is_none());
    let (rest, output) = d.into_inner();
    assert_eq!(rest, &b[end..]);
    assert_eq!(&output[6..], [999; 12]);
    for len in end - 16..end {
        assert!(decode(&b[..len], 1).is_err());
    }

    #[cfg(feature = "std")]
    {
        let mut d = h4m::AudioDecoder::new(b.as_slice(), 1).unwrap();
        let mut actual = Vec::new();
        while let Some(pcm) = d.next_block().unwrap() {
            actual.extend_from_slice(pcm);
        }
        assert_eq!(actual, decode(&b, 1).unwrap());
        assert_eq!(d.into_inner(), &b[end..]);
    }
}

#[cfg(feature = "std")]
#[test]
fn streaming_wrapper_agrees_with_slice_and_packet_decoders() {
    use std::io::{self, Read};
    // Force short reads through read_exact for headers and compressed packets.
    struct ShortReads<'a>(&'a [u8]);
    impl Read for ShortReads<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let size = buffer.len().min(1);
            self.0.read(&mut buffer[..size])
        }
    }
    let b = fixture();
    for track in [1, 2] {
        let mut d = h4m::AudioDecoder::new(ShortReads(&b), track).unwrap();
        let mut packet = AudioPacketDecoder::new(AudioInfo::parse(&b, track).unwrap()).unwrap();
        let mut pcm = [0; 18];
        assert_eq!(d.metadata(), packet.metadata());
        assert_eq!(
            d.next_block().unwrap().unwrap(),
            packet
                .decode_into(h4m::AudioPacketMode::Initialize, &b[96..118], &mut pcm)
                .unwrap()
        );
        assert_eq!(
            d.next_block().unwrap().unwrap(),
            packet
                .decode_into(h4m::AudioPacketMode::Continue, &b[126..134], &mut pcm)
                .unwrap()
        );
        assert!(d.next_block().unwrap().is_none());
        assert!(d.into_inner().0.is_empty());
    }
    for len in 0..b.len() {
        let result = (|| -> Result<(), Error> {
            let mut d = h4m::AudioDecoder::new(&b[..len], 1)?;
            while d.next_block()?.is_some() {}
            Ok(())
        })();
        assert!(result.is_err(), "streaming prefix {len}");
    }
    let mut bad = b;
    bad[91] = 2;
    let mut d = h4m::AudioDecoder::new(bad.as_slice(), 1).unwrap();
    assert!(d.next_block().is_err());
    assert!(matches!(d.next_block(), Err(Error::Failed)));
}

#[test]
fn policies_accept_or_reject_requirements_without_changing_them() {
    let b = fixture();
    let info = AudioInfo::parse(&b, 1).unwrap();
    let required = info.buffer_requirements();
    let exact = AudioLimits {
        max_packet_bytes: required.packet_bytes,
        max_frames_per_packet: required.pcm_samples / 2,
    };
    // The first packet only needs three frames, but requirements reserve nine.
    for limits in [
        AudioLimits {
            max_packet_bytes: exact.max_packet_bytes - 1,
            ..exact
        },
        AudioLimits {
            max_frames_per_packet: exact.max_frames_per_packet - 1,
            ..exact
        },
    ] {
        let mut storage = [777; 18];
        assert!(matches!(info.validate_limits(limits), Err(Error::Limit(_))));
        assert!(matches!(
            SliceAudioDecoder::with_buffer_and_limits(&b, 1, &mut storage[..], limits),
            Err(Error::Limit(_))
        ));
        assert_eq!(storage, [777; 18]);
        assert_eq!(info.buffer_requirements(), required);
        #[cfg(feature = "std")]
        assert!(matches!(
            h4m::AudioDecoder::with_limits(b.as_slice(), 1, limits),
            Err(Error::Limit(_))
        ));
    }
    for limits in [exact, AudioLimits::default()] {
        info.validate_limits(limits).unwrap();
        let mut d = SliceAudioDecoder::with_buffer_and_limits(&b, 1, [0; 18], limits).unwrap();
        assert_eq!(d.buffer_requirements(), required);
        assert_eq!(
            d.next_block().unwrap().unwrap(),
            [200, 100, 201, 101, 200, 100]
        );
        assert_eq!(d.next_block().unwrap().unwrap(), [201, 101, 202, 102]);
        assert!(d.next_block().unwrap().is_none());
    }
}
