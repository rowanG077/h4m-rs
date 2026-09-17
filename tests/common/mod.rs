#![allow(dead_code)]
// Small test encoder: explicit Huffman trees and deterministic, synthetic images.

use h4m::{FrameType, Version, VideoInfo};

#[derive(Default, Clone)]
struct Bits {
    bytes: Vec<u8>,
    count: usize,
}

impl Bits {
    fn write(&mut self, value: u32, bits: usize) {
        for shift in (0..bits).rev() {
            if self.count.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let end = self.bytes.len() - 1;
            self.bytes[end] |= (((value >> shift) & 1) as u8) << (7 - self.count % 8);
            self.count += 1;
        }
    }

    fn symbol(&mut self, value: u8) {
        self.write(u32::from(value), 8);
    }

    fn tree(&mut self, first: u32, count: u32) {
        if count == 1 {
            self.write(0, 1);
            self.write(first, 8);
        } else {
            self.write(1, 1);
            self.tree(first, count / 2);
            self.tree(first + count / 2, count / 2);
        }
    }

    fn signed(&mut self, mut value: i32) {
        while value <= -128 {
            self.symbol(128);
            value += 128;
        }
        while value >= 127 {
            self.symbol(127);
            value -= 127;
        }
        self.symbol(value as u8);
    }

    fn run(&mut self, mut value: usize) {
        while value >= 255 {
            self.symbol(255);
            value -= 255;
        }
        self.symbol(value as u8);
    }
}

pub struct Packet {
    pub kind: FrameType,
    pub display: u32,
    pub data: Vec<u8>,
}

pub struct Fixture {
    pub name: String,
    pub info: VideoInfo,
    pub packets: Vec<Packet>,
}

fn streams(inter: bool) -> [Bits; 17] {
    let mut s: [Bits; 17] = std::array::from_fn(|_| Bits::default());
    for i in [0, 1, 4, 5] {
        s[i].tree(0, 256);
    }
    if inter {
        s[13].tree(0, 256);
        s[15].tree(0, 256);
    }
    s
}

fn packet(s: [Bits; 17], inter: bool, shift: u8, residual: u8) -> Vec<u8> {
    let mut result = vec![shift, 12, 0, 0, 0, 0, 0, 0];
    if inter {
        result[2..6].fill(residual);
    }
    let count = if inter { 17 } else { 16 };
    let mut payload = Vec::new();
    for bits in &s[..count] {
        result.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        // The original reader refills four bytes at a time, hence word padding.
        let mut bytes = bits.bytes.clone();
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        payload.extend_from_slice(&bytes);
    }
    result.extend_from_slice(&payload);
    result
}

fn dims(info: VideoInfo, plane: usize) -> (usize, usize) {
    let hs = if plane == 0 {
        1
    } else {
        info.horizontal_sampling as usize
    };
    let vs = if plane == 0 {
        1
    } else {
        info.vertical_sampling as usize
    };
    (info.width as usize / hs / 4, info.height as usize / vs / 4)
}

fn type_symbol(s: &mut [Bits; 17], group: usize, value: u8) {
    s[group * 2].symbol(value);
    if value == 0 {
        s[group * 2 + 1].symbol(0);
    }
}

fn detail(s: &mut [Bits; 17], plane: usize, kind: u8, seed: usize, predicted: bool) {
    if kind == 6 {
        for n in 0..16 {
            s[6 + plane * 3].symbol(((seed * 37 + n * 19 + plane * 71) % 256) as u8);
        }
    } else if (1..=5).contains(&kind) {
        let bases = kind - u8::from(predicted);
        for b in 0..bases {
            s[5 + plane * 3].symbol(((seed + b as usize) % 23) as u8);
            let code = if predicted {
                // MC nest is centered 32,16 pixels before the reference block.
                32 | (16 << 6) | (((seed + b as usize) & 1) << 15)
            } else {
                ((seed * 3 + b as usize * 13) % 64)
                    | (((seed * 7 + b as usize) % 32) << 6)
                    | ((seed & 3) << 11)
                    | (((seed + b as usize) & 7) << 13)
            };
            s[6 + plane * 3].symbol((code >> 8) as u8);
            s[6 + plane * 3].symbol(code as u8);
        }
        if predicted {
            s[4 + plane * 3].signed((seed as i32 % 11) - 5);
            s[4 + plane * 3].signed((seed as i32 % 19) - 9);
        }
    }
}

pub fn intra(info: VideoInfo, mode: usize, seed: usize, shift: u8) -> Vec<u8> {
    let mut s = streams(false);
    let mut types: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
    for (plane, plane_types) in types.iter_mut().enumerate() {
        let (w, h) = dims(info, plane);
        for n in 0..w * h {
            let kind = match mode {
                0 => 0,
                1 => 8,
                2 => 6,
                _ => [0, 8, 6, 1, 2, 3, 4, 5][(n + plane * 3 + seed) % 8],
            };
            plane_types.push(kind);
        }
    }
    for &v in &types[0] {
        type_symbol(&mut s, 0, v);
    }
    for (&u, &v) in types[1].iter().zip(&types[2]) {
        type_symbol(&mut s, 1, u | (v << 4));
    }
    for (plane, plane_types) in types.iter().enumerate() {
        let (w, h) = dims(info, plane);
        let mut dc = vec![127u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let n = y * w + x;
                let top = if y == 0 { 127 } else { dc[n - w] };
                let pred = if x == 0 {
                    top
                } else {
                    (u16::from(dc[n - 1]) + u16::from(top)).div_ceil(2) as u8
                };
                let delta = ((n * 17 + plane * 23 + seed * 3) % 256) as u8 as i8 as i32;
                s[4 + plane * 3].signed(delta);
                if delta == 0 {
                    s[13 + plane].symbol(0);
                }
                dc[n] = pred.wrapping_add((delta << shift) as u8);
            }
        }
        for (n, &kind) in plane_types.iter().enumerate() {
            detail(&mut s, plane, kind, n + seed, false);
        }
    }
    packet(s, false, shift, 0)
}

fn runs(bits: &mut Bits, values: &[u8], initial_bits: usize) {
    if values.is_empty() {
        return;
    }
    bits.write(u32::from(values[0]), initial_bits);
    let mut start = 0;
    while start < values.len() {
        let mut end = start + 1;
        while end < values.len() && values[end] == values[start] {
            end += 1;
        }
        bits.run(end - start);
        if end < values.len() && initial_bits == 2 {
            bits.write(u32::from(values[end] != (values[start] + 1) % 3), 1);
        }
        start = end;
    }
}

pub fn inter(info: VideoInfo, bframe: bool, mode: usize, seed: usize, residual: u8) -> Vec<u8> {
    let mut s = streams(true);
    let mw = info.width as usize / 8;
    let mh = info.height as usize / 8;
    let mut types = Vec::new();
    let mut procs = Vec::new();
    let mut desc = Vec::new();
    for n in 0..mw * mh {
        let target = if mode == 4 {
            2
        } else if mode == 0 {
            1
        } else if mode == 1 || n % 7 == 2 {
            0
        } else if bframe && n % 3 != 0 {
            2
        } else {
            1
        };
        let proc = if mode == 0 || mode == 4 {
            1
        } else if mode == 2 {
            0
        } else {
            u8::from(n % 3 == 0)
        };
        types.push(target);
        if target != 0 {
            procs.push(proc);
        }
        let mut blocks: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
        for (plane, plane_blocks) in blocks.iter_mut().enumerate() {
            let count = if plane == 0 {
                4
            } else {
                4 / (info.horizontal_sampling as usize * info.vertical_sampling as usize)
            };
            for sub in 0..count {
                let kind = if target != 0 && proc == 1 {
                    0
                } else if target == 0 {
                    [0, 8, 6, 1, 2, 3, 4, 5][(n + sub + plane + seed) % 8]
                } else {
                    [0, 6, 1, 2, 3, 4, 5][(n + sub + plane + seed) % 7]
                };
                plane_blocks.push(kind);
                if target == 0 {
                    s[4 + plane * 3].signed(((n + sub + plane) % 31) as i32 - 15);
                }
            }
        }
        if target == 0 || proc == 0 {
            for &v in &blocks[0] {
                type_symbol(&mut s, 0, v);
            }
            for (&u, &v) in blocks[1].iter().zip(&blocks[2]) {
                type_symbol(&mut s, 1, u | (v << 4));
            }
        }
        desc.push((target, blocks));
    }
    runs(&mut s[15], &types, 2);
    runs(&mut s[16], &procs, 1);
    let mut reference = 3;
    let mut mv = [0i32; 2];
    for (n, (target, blocks)) in desc.iter().enumerate() {
        if *target != 0 {
            if reference != *target {
                reference = *target;
                mv = [0; 2];
            }
            let x = n % mw;
            let y = n / mw;
            let desired = if mode == 0 || mode == 2 || mode == 4 {
                [0, 0]
            } else if mode >= 5 {
                [
                    if mode == 6 {
                        0
                    } else if x == mw - 1 {
                        -3
                    } else {
                        3
                    },
                    if mode == 5 {
                        0
                    } else if y == mh - 1 {
                        -3
                    } else {
                        3
                    },
                ]
            } else {
                [
                    if x == mw - 1 { -4 } else { 3 },
                    if y == mh - 1 { -4 } else { 3 },
                ]
            };
            for axis in 0..2 {
                let delta = desired[axis] - mv[axis];
                s[13 + axis].symbol((delta >> residual) as u8);
                s[13 + axis].write((delta & ((1 << residual) - 1)) as u32, residual as usize);
                mv[axis] = desired[axis];
            }
        }
        for (plane, plane_blocks) in blocks.iter().enumerate() {
            for (sub, &kind) in plane_blocks.iter().enumerate() {
                detail(&mut s, plane, kind, n + sub + seed, *target != 0);
            }
        }
    }
    packet(s, true, 0, residual)
}

pub fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    for version in [Version::V13, Version::V15] {
        for (width, height) in [(16, 16), (32, 16), (16, 32), (320, 168)] {
            let info = VideoInfo {
                version,
                width,
                height,
                horizontal_sampling: 2,
                vertical_sampling: 2,
            };
            for mode in 0..4 {
                let packets = vec![Packet {
                    kind: FrameType::I,
                    display: 0,
                    data: intra(info, mode, 13, (mode % 3) as u8),
                }];
                out.push(Fixture {
                    name: format!("{version:?}-{width}x{height}-intra-{mode}"),
                    info,
                    packets,
                });
            }
            for mode in 0..4 {
                let packets = vec![
                    Packet {
                        kind: FrameType::I,
                        display: 0,
                        data: intra(info, 3, 7, 0),
                    },
                    Packet {
                        kind: FrameType::P,
                        display: 3,
                        data: inter(info, false, mode, 11, 1),
                    },
                    Packet {
                        kind: FrameType::B,
                        display: 1,
                        data: inter(info, true, mode, 17, 2),
                    },
                    Packet {
                        kind: FrameType::B,
                        display: 2,
                        data: inter(info, true, mode, 23, 0),
                    },
                    Packet {
                        kind: FrameType::P,
                        display: 4,
                        data: inter(info, false, mode, 31, 0),
                    },
                ];
                out.push(Fixture {
                    name: format!("{version:?}-{width}x{height}-inter-{mode}"),
                    info,
                    packets,
                });
            }
        }
    }
    for version in [Version::V13, Version::V15] {
        for (hs, vs) in [(1, 1), (2, 1)] {
            let info = VideoInfo {
                version,
                width: 32,
                height: 32,
                horizontal_sampling: hs,
                vertical_sampling: vs,
            };
            for mode in 0..4 {
                out.push(Fixture {
                    name: format!("{version:?}-sampling-{hs}-{vs}-mode-{mode}"),
                    info,
                    packets: vec![
                        Packet {
                            kind: FrameType::I,
                            display: 0,
                            data: intra(info, 3, 2, 0),
                        },
                        Packet {
                            kind: FrameType::P,
                            display: 2,
                            data: inter(info, false, mode, 11, 0),
                        },
                        Packet {
                            kind: FrameType::B,
                            display: 1,
                            data: inter(info, true, mode, 17, 0),
                        },
                    ],
                });
            }
        }
        let info = VideoInfo {
            version,
            width: 320,
            height: 168,
            horizontal_sampling: 2,
            vertical_sampling: 2,
        };
        out.push(Fixture {
            name: format!("{version:?}-constant-trees-runs"),
            info,
            packets: vec![Packet {
                kind: FrameType::I,
                display: 0,
                data: constant_intra(),
            }],
        });
    }
    for version in [Version::V13, Version::V15] {
        for (hs, vs) in [(1, 1), (2, 1), (2, 2)] {
            let info = VideoInfo {
                version,
                width: 32,
                height: 32,
                horizontal_sampling: hs,
                vertical_sampling: vs,
            };
            for mode in 2..8 {
                let mut packets: Vec<_> = [11, 19, 31]
                    .into_iter()
                    .enumerate()
                    .map(|(index, seed)| Packet {
                        kind: FrameType::I,
                        display: index as u32,
                        data: intra(info, 2, seed, 0),
                    })
                    .collect();
                for index in 3..6 {
                    packets.push(Packet {
                        kind: FrameType::P,
                        display: index,
                        // Encode second-reference selectors in a P picture.
                        data: inter(info, true, mode, index as usize * 7, 1),
                    });
                }
                out.push(Fixture {
                    name: format!("{version:?}-p-current-{hs}-{vs}-mode-{mode}"),
                    info,
                    packets,
                });
            }
        }
    }
    out
}

// All zero basis/DC symbols, each followed by 255 repeated symbols. No bits are
// consumed by singleton trees; chroma and DC secondary streams are absent.
fn constant_intra() -> Vec<u8> {
    let mut s: [Bits; 17] = std::array::from_fn(|_| Bits::default());
    for (stream, value) in [(0, 0), (1, 255), (4, 0), (5, 0)] {
        s[stream].write(0, 1);
        s[stream].write(value, 8);
    }
    packet(s, false, 0, 0)
}

fn kind(kind: FrameType) -> u16 {
    match kind {
        FrameType::I => 0x10,
        FrameType::P => 0x20,
        FrameType::B => 0x30,
    }
}

pub fn container(fixture: &Fixture, gops: u32, audio: bool) -> Vec<u8> {
    let mut body = Vec::new();
    let mut previous = 0u32;
    for _ in 0..gops {
        let mut block = Vec::new();
        if audio {
            block.extend_from_slice(&0u16.to_be_bytes());
            block.extend_from_slice(&0u16.to_be_bytes());
            block.extend_from_slice(&8u32.to_be_bytes());
            block.extend_from_slice(&[0; 8]);
        }
        for p in &fixture.packets {
            block.extend_from_slice(&1u16.to_be_bytes());
            block.extend_from_slice(&kind(p.kind).to_be_bytes());
            block.extend_from_slice(&(p.data.len() as u32 + 4).to_be_bytes());
            block.extend_from_slice(&p.display.to_be_bytes());
            block.extend_from_slice(&p.data);
        }
        body.extend_from_slice(&previous.to_be_bytes());
        body.extend_from_slice(&(block.len() as u32).to_be_bytes());
        body.extend_from_slice(&(fixture.packets.len() as u32).to_be_bytes());
        body.extend_from_slice(&u32::from(audio).to_be_bytes());
        body.extend_from_slice(&0x01000000u32.to_be_bytes());
        previous = block.len() as u32 + 20;
        body.extend_from_slice(&block);
    }
    let mut header = vec![0u8; 68];
    header[..9].copy_from_slice(if fixture.info.version == Version::V13 {
        b"HVQM4 1.3"
    } else {
        b"HVQM4 1.5"
    });
    for (offset, value) in [
        (16, 68),
        (20, body.len() as u32),
        (24, gops),
        (28, fixture.packets.len() as u32 * gops),
        (32, u32::from(audio) * gops),
        (36, 33367),
        (
            40,
            fixture
                .packets
                .iter()
                .map(|p| p.data.len() as u32 + 4)
                .max()
                .unwrap(),
        ),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    header[52..54].copy_from_slice(&fixture.info.width.to_be_bytes());
    header[54..56].copy_from_slice(&fixture.info.height.to_be_bytes());
    header[56] = fixture.info.horizontal_sampling;
    header[57] = fixture.info.vertical_sampling;
    header[60] = 1;
    header[61] = 16;
    header[64..68].copy_from_slice(&32000u32.to_be_bytes());
    header.extend_from_slice(&body);
    header
}
