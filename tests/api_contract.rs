//! Public API contracts and boundary regressions.

#[path = "common/audio.rs"]
mod audio;
mod common;
use h4m::{
    BlockState, FrameType, SliceVideoDecoder, VideoBuffers, VideoLimits, VideoPacketDecoder,
};

#[test]
fn packet_limit_means_the_same_in_raw_and_container_apis() {
    let mut fixture = common::fixtures().remove(0);
    fixture.packets.truncate(1);
    let limit = fixture.packets[0].data.len();
    let limits = VideoLimits {
        max_frame_bytes: limit,
        ..Default::default()
    };
    let buffers = || VideoBuffers {
        frames: [[0; 384]; 3],
        blocks: [BlockState::EMPTY; 68],
    };
    let mut raw =
        VideoPacketDecoder::with_buffers_and_limits(fixture.info, buffers(), limits).unwrap();
    raw.decode(FrameType::I, 0, &fixture.packets[0].data)
        .unwrap();
    let input = common::container(&fixture, 1, false);
    let mut slice = SliceVideoDecoder::with_buffers_and_limits(&input, buffers(), limits).unwrap();
    slice.next_frame().unwrap().unwrap();
}

#[test]
fn malformed_planes_are_rejected_before_a_frame_can_exist() {
    let info = common::fixtures().remove(0).info;
    assert!(h4m::Frame::from_planes(info, FrameType::I, 0, [&[0; 256], &[], &[0; 64]]).is_err());
    let frame = h4m::Frame::from_planes(info, FrameType::I, 0, [&[0; 256], &[128; 64], &[128; 64]])
        .unwrap();
    assert_eq!(frame.rgb_buffer_size(), 768);
    frame.to_rgb_into(&mut [0; 768]).unwrap();
}

#[test]
fn header_audio_format_is_not_a_channel_count() {
    let mut input = audio::fixture();
    let header = h4m::Header::parse(&input[..h4m::Header::SIZE]).unwrap();
    assert_eq!(header.audio_format(), 2);
    assert_eq!(header.audio_bits(), 16);
    assert_eq!(header.audio_flags(), 0);
    assert_eq!(header.audio_tracks(), 2);
    assert_eq!(header.audio_packets(), 2);
    assert_eq!(
        header.audio_info(2).unwrap(),
        h4m::AudioInfo::parse(&input, 2).unwrap()
    );
    assert_eq!(header.audio_info(2).unwrap().channels(), 2);
    // AFC format is valid container metadata, but unsupported by the IMA decoder.
    input[60] = 4;
    let header = h4m::Header::parse(&input).unwrap();
    assert_eq!(header.audio_format(), 4);
    assert!(header.audio_info(1).is_err());
    assert!(header.audio_info(0).is_err());
}

#[test]
fn audio_modes_convert_at_the_wire_boundary() {
    use h4m::AudioPacketMode::{Continue, Initialize};
    for (wire, mode) in [(2, Continue), (3, Initialize)] {
        assert_eq!(h4m::AudioPacketMode::try_from(wire).unwrap(), mode);
        assert_eq!(mode as u16, wire);
    }
    for wire in [0, 1, 4, u16::MAX] {
        assert!(h4m::AudioPacketMode::try_from(wire).is_err());
    }
}

#[test]
fn default_constructors_match_explicit_policy_and_expose_metadata() {
    let input = audio::fixture();
    let mut a = h4m::SliceAudioDecoder::with_buffer(&input, 2, [0; 18]).unwrap();
    let mut b = h4m::SliceAudioDecoder::with_buffer_and_limits(
        &input,
        2,
        [0; 18],
        h4m::AudioLimits::default(),
    )
    .unwrap();
    assert_eq!(a.metadata(), b.metadata());
    while let Some(pcm) = a.next_block().unwrap() {
        assert_eq!(pcm, b.next_block().unwrap().unwrap());
    }
    assert!(b.next_block().unwrap().is_none());

    let fixture = common::fixtures().remove(0);
    let input = common::container(&fixture, 1, false);
    let buffers = || VideoBuffers {
        frames: [[0; 384]; 3],
        blocks: [BlockState::EMPTY; 68],
    };
    let mut a = SliceVideoDecoder::with_buffers(&input, buffers()).unwrap();
    let mut b =
        SliceVideoDecoder::with_buffers_and_limits(&input, buffers(), VideoLimits::default())
            .unwrap();
    let mut packet = VideoPacketDecoder::with_buffers(fixture.info, buffers()).unwrap();
    assert_eq!(a.metadata(), fixture.info);
    assert_eq!(a.metadata(), b.metadata());
    assert_eq!(packet.metadata(), a.metadata());
    for p in &fixture.packets {
        let a = a.next_frame().unwrap().unwrap();
        let b = b.next_frame().unwrap().unwrap();
        let raw = packet.decode(p.kind, p.display, &p.data).unwrap();
        for ((a, b), raw) in a.planes().into_iter().zip(b.planes()).zip(raw.planes()) {
            assert_eq!(a.data(), b.data());
            assert_eq!(a.data(), raw.data());
        }
    }
    assert!(a.next_frame().unwrap().is_none());
    assert!(b.next_frame().unwrap().is_none());
}

#[test]
fn rgb_requirements_cover_every_sampling_layout_and_preserve_unused_storage() {
    use h4m::{ChromaSampling::*, Error, Frame, Version, VideoInfo};
    for sampling in [Yuv444, Yuv422, Yuv420] {
        let info = VideoInfo::new(Version::V15, 16, 8, sampling).unwrap();
        let chroma_len = 128
            / usize::from(sampling.horizontal_factor())
            / usize::from(sampling.vertical_factor());
        let chroma = vec![128; chroma_len];
        let frame =
            Frame::from_planes(info, FrameType::I, 7, [&[23; 128], &chroma, &chroma]).unwrap();
        assert_eq!(frame.display_index(), 7);
        assert_eq!(frame.y().width() * frame.y().height(), 128);
        assert_eq!(frame.u().width() * frame.u().height(), chroma_len);
        assert_eq!(frame.rgb_buffer_size(), info.rgb_buffer_size());
        let mut rgb = vec![0xa5; info.rgb_buffer_size() + 3];
        assert!(matches!(
            frame.to_rgb_into(&mut rgb[..383]),
            Err(Error::BufferTooSmall {
                required: 384,
                provided: 383,
                ..
            })
        ));
        assert!(rgb.iter().all(|&v| v == 0xa5));
        frame.to_rgb_into(&mut rgb).unwrap();
        assert!(rgb[..384].iter().all(|&v| v == 23));
        assert_eq!(&rgb[384..], &[0xa5; 3]);
        assert!(Frame::from_planes(info, FrameType::I, 0, [&[0; 129], &chroma, &chroma]).is_err());
        #[cfg(feature = "alloc")]
        {
            let mut allocated = vec![0; 600];
            frame.to_rgb(&mut allocated).unwrap();
            assert_eq!(allocated, rgb[..384]);
        }
    }
}

// Safe user-defined storage may expose shorter views on later calls. The API
// must return an error instead of panicking, even when its stability contract is violated.
struct ShrinkingFrame {
    data: [u8; 384],
    calls: usize,
}

impl AsRef<[u8]> for ShrinkingFrame {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl AsMut<[u8]> for ShrinkingFrame {
    fn as_mut(&mut self) -> &mut [u8] {
        self.calls += 1;
        let size = if self.calls == 1 { 384 } else { 0 };
        &mut self.data[..size]
    }
}

#[test]
fn shrinking_custom_storage_returns_an_error_instead_of_panicking() {
    let fixture = common::fixtures().remove(0);
    let mut decoder = VideoPacketDecoder::with_buffers(
        fixture.info,
        VideoBuffers {
            frames: core::array::from_fn(|_| ShrinkingFrame {
                data: [0; 384],
                calls: 0,
            }),
            blocks: [BlockState::EMPTY; 68],
        },
    )
    .unwrap();
    let p = &fixture.packets[0];
    assert!(matches!(
        decoder.decode(p.kind, p.display, &p.data),
        Err(h4m::Error::BufferTooSmall { provided: 0, .. })
    ));
    assert!(matches!(
        decoder.decode(p.kind, p.display, &p.data),
        Err(h4m::Error::Failed)
    ));
}
