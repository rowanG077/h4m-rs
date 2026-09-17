//! Synthetic stereo IMA containers.

pub fn packet(mode: u16, samples: u32, tracks: &[&[u8]]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(0u16.to_be_bytes());
    p.extend(mode.to_be_bytes());
    p.extend((4 + tracks.iter().map(|t| t.len()).sum::<usize>() as u32).to_be_bytes());
    p.extend(samples.to_be_bytes());
    for t in tracks {
        p.extend(*t);
    }
    p
}

pub fn fixture() -> Vec<u8> {
    let first = packet(
        3,
        3,
        &[
            &[0, 100, 0, 0, 200, 0, 0x11, 0x99, 0xff],
            &[0, 10, 0, 0, 20, 0, 0x77, 0xff, 0],
        ],
    );
    let second = packet(2, 2, &[&[0x11, 0x11], &[0, 0]]);
    let mut b = vec![0; 68];
    b[..16].copy_from_slice(b"HVQM4 1.5\0\0\0\0\0\0\0");
    for (at, v) in [
        (16, 68),
        (20, 20 + first.len() as u32 + second.len() as u32),
        (24, 1),
        (32, 2),
        (40, 64),
        (48, 22),
        (64, 32028),
    ] {
        b[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    b[52..56].copy_from_slice(&[0, 16, 0, 16]);
    b[56..58].copy_from_slice(&[2, 2]);
    b[60..64].copy_from_slice(&[2, 16, 0, 1]);
    for v in [0, (first.len() + second.len()) as u32, 0, 2, 0x01000000] {
        b.extend(v.to_be_bytes());
    }
    b.extend(first);
    b.extend(second);
    b
}
