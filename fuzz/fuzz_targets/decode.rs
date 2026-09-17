#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = h4m::AudioLimits {
        max_packet_bytes: 4096,
        max_frames_per_packet: 2048,
    };
    let mut pcm = [0i16; 4096];
    for track in [1, 2] {
        if let Ok(mut decoder) =
            h4m::SliceAudioDecoder::with_buffer_and_limits(data, track, &mut pcm[..], limits)
        {
            // Bound work independently of attacker-controlled duration fields.
            for _ in 0..32 {
                match decoder.next_block() {
                    Ok(Some(_)) => {}
                    _ => break,
                }
            }
        }
        if let Ok(mut decoder) = h4m::AudioInfo::parse(data, track)
            .and_then(|info| h4m::AudioPacketDecoder::with_limits(info, limits))
        {
            let packet = &data[68..];
            let _ = decoder.decode_into(h4m::AudioPacketMode::Initialize, packet, &mut pcm);
            let before = pcm;
            // Force a buffer error; packet errors must not modify caller storage.
            if decoder
                .decode_into(h4m::AudioPacketMode::Continue, packet, &mut pcm[..1])
                .is_err()
            {
                assert_eq!(pcm, before);
            }
            let _ = decoder.decode_into(h4m::AudioPacketMode::Continue, packet, &mut pcm);
        }
    }
});
