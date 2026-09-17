use crate::{
    entropy::{Runs, Streams},
    error::{be16, Error, Result},
    Frame, FrameType, Limits, Version, VideoInfo,
};

#[derive(Clone, Copy)]
struct Block {
    dc: u8,
    kind: u8,
}

struct Plane {
    width: usize,
    height: usize,
    offset: usize,
    sx: u8,
    sy: u8,
    bw: usize,
    bh: usize,
    stride: usize,
    blocks: Vec<Block>,
}

impl Plane {
    fn new(info: VideoInfo, index: usize, offset: usize) -> Self {
        let sx = u8::from(index != 0 && info.horizontal_sampling == 2);
        let sy = u8::from(index != 0 && info.vertical_sampling == 2);
        let width = usize::from(info.width) >> sx;
        let height = usize::from(info.height) >> sy;
        let bw = width / 4;
        let bh = height / 4;
        Self {
            width,
            height,
            offset,
            sx,
            sy,
            bw,
            bh,
            stride: bw + 2,
            blocks: vec![Block { dc: 127, kind: 255 }; (bw + 2) * (bh + 2)],
        }
    }

    fn index(&self, x: usize, y: usize) -> usize {
        (y + 1) * self.stride + x + 1
    }

    fn macroblock(&self, x: usize, y: usize, sub: usize) -> (usize, usize) {
        const XY: [(usize, usize); 4] = [(0, 0), (0, 1), (1, 1), (1, 0)];
        (
            ((x * 2) >> self.sx) + XY[sub].0,
            ((y * 2) >> self.sy) + XY[sub].1,
        )
    }

    fn count(&self) -> usize {
        4 >> (self.sx + self.sy)
    }

    fn put(&self, dst: &mut [u8], x: usize, y: usize, block: &[u8; 16]) {
        let start = self.offset + y * 4 * self.width + x * 4;
        for row in 0..4 {
            dst[start + row * self.width..start + row * self.width + 4]
                .copy_from_slice(&block[row * 4..row * 4 + 4]);
        }
    }
}

/// Stateful decoder for demuxed HVQM4 video packets.
///
/// Packets must be supplied in decoding order. The four-byte display ID is
/// supplied separately; `packet` starts at the DC/transform shift bytes.
/// Three frame buffers are reused; returned frames borrow the decoder.
pub struct VideoDecoder {
    info: VideoInfo,
    planes: [Plane; 3],
    frames: [Vec<u8>; 3],
    current: usize,
    past: usize,
    future: usize,
    references: u8,
    nest: [u8; 70 * 38],
    failed: bool,
    max_packet: usize,
}

impl VideoDecoder {
    /// Create a decoder with the default resource limits.
    pub fn new(info: VideoInfo) -> Result<Self> {
        Self::with_limits(info, Limits::default())
    }

    /// Create a decoder with explicit resource limits.
    pub fn with_limits(info: VideoInfo, limits: Limits) -> Result<Self> {
        info.validate(limits)?;
        let mut offset = 0;
        let planes = std::array::from_fn(|i| {
            let p = Plane::new(info, i, offset);
            offset += p.width * p.height;
            p
        });
        Ok(Self {
            info,
            planes,
            frames: std::array::from_fn(|_| vec![0; offset]),
            current: 0,
            past: 1,
            future: 2,
            references: 0,
            nest: [0; 70 * 38],
            failed: false,
            max_packet: limits.max_frame_bytes,
        })
    }

    /// The dimensions, sampling, and version used by this decoder.
    pub fn info(&self) -> VideoInfo {
        self.info
    }

    /// Decode a packet, returning its planar pixels without copying.
    ///
    /// A decoding error invalidates reference state; subsequent calls return
    /// [`Error::Failed`]. Start a new decoder after an error.
    pub fn decode(
        &mut self,
        kind: FrameType,
        display_index: u32,
        packet: &[u8],
    ) -> Result<Frame<'_>> {
        if self.failed {
            return Err(Error::Failed);
        }
        self.failed = true;
        if packet.len() > self.max_packet {
            return Err(Error::Limit("compressed frame size"));
        }
        if kind != FrameType::I && self.references == 0 {
            return Err(Error::Invalid("prediction before an I frame"));
        }
        if kind == FrameType::B && self.references < 2 {
            return Err(Error::Invalid("B frame needs past and future references"));
        }
        if kind != FrameType::B {
            std::mem::swap(&mut self.past, &mut self.future);
        }
        let mut streams = Streams::new(packet, kind != FrameType::I)?;
        if kind == FrameType::I {
            self.intra(
                &mut streams,
                be16(packet, 4)? as usize,
                be16(packet, 6)? as usize,
            )?;
        } else {
            self.inter(&mut streams, kind)?;
        }
        let output = self.current;
        if kind != FrameType::B {
            std::mem::swap(&mut self.current, &mut self.future);
            self.references = (self.references + 1).min(2);
        }
        self.failed = false;
        Ok(Frame::new(
            self.info,
            kind,
            display_index,
            &self.frames[output],
        ))
    }

    fn intra(&mut self, streams: &mut Streams<'_>, nest_x: usize, nest_y: usize) -> Result<()> {
        for group in 0..2 {
            let mut run = 0;
            let p = &self.planes[group];
            let (bw, bh) = (p.bw, p.bh);
            for y in 0..bh {
                for x in 0..bw {
                    let value = streams.block_types(group == 1, &mut run)?;
                    let index = self.planes[group].index(x, y);
                    self.planes[group].blocks[index].kind =
                        if group == 0 { value } else { value & 15 };
                    if group == 1 {
                        self.planes[2].blocks[index].kind = value >> 4;
                    }
                }
            }
        }
        for (i, p) in self.planes.iter_mut().enumerate() {
            let mut run = 0;
            for y in 0..p.bh {
                let mut prediction = p.blocks[p.index(0, y) - p.stride].dc;
                for x in 0..p.bw {
                    let index = p.index(x, y);
                    let dc = prediction.wrapping_add(streams.delta(i, &mut run)? as u8);
                    p.blocks[index].dc = dc;
                    prediction = (u16::from(dc) + u16::from(p.blocks[index - p.stride + 1].dc))
                        .div_ceil(2) as u8;
                }
            }
        }
        self.make_nest(nest_x, nest_y)?;
        let landscape = self.info.width >= self.info.height;
        for (i, p) in self.planes.iter().enumerate() {
            for y in 0..p.bh {
                for x in 0..p.bw {
                    let index = p.index(x, y);
                    let block = p.blocks[index];
                    let pixels = if block.kind == 0 {
                        let top = p.index(x, y.saturating_sub(1));
                        let bottom = p.index(x, (y + 1).min(p.bh - 1));
                        let left = p.index(x.saturating_sub(1), y);
                        let right = p.index((x + 1).min(p.bw - 1), y);
                        weighted(
                            block.dc,
                            [top, bottom, left, right].map(|n| neighbor(&p.blocks, n, block.dc)),
                        )
                    } else {
                        intra_block(streams, i, block, &self.nest, landscape)?
                    };
                    p.put(&mut self.frames[self.current], x, y, &pixels);
                }
            }
        }
        Ok(())
    }

    fn make_nest(&mut self, x: usize, y: usize) -> Result<()> {
        let p = &self.planes[0];
        let (nw, nh) = if self.info.width >= self.info.height {
            (70, 38)
        } else {
            (38, 70)
        };
        let w = p.bw.min(nw);
        let h = p.bh.min(nh);
        if x + w > p.bw || y + h > p.bh {
            return Err(Error::Invalid("DC nest outside luma plane"));
        }
        self.nest.fill(0);
        for ny in 0..nh.min(h * 2) {
            let source_y = if ny < h { ny } else { h * 2 - 1 - ny };
            for nx in 0..nw.min(w * 2) {
                let source_x = if nx < w { nx } else { w * 2 - 1 - nx };
                self.nest[ny * nw + nx] = p.blocks[p.index(x + source_x, y + source_y)].dc >> 4;
            }
        }
        Ok(())
    }

    fn descriptors(&mut self, streams: &mut Streams<'_>) -> Result<()> {
        let mut types = Runs::new(streams, 15, 2)?;
        let mut procs = Runs::new(streams, 16, 1)?;
        let mut dc = [127u8; 3];
        let mut runs = [0; 2];
        for my in 0..usize::from(self.info.height) / 8 {
            for mx in 0..usize::from(self.info.width) / 8 {
                let kind = types.next(streams)?;
                let proc = if kind == 0 { 0 } else { procs.next(streams)? };
                if kind == 0 {
                    for (i, p) in self.planes.iter_mut().enumerate() {
                        for sub in 0..p.count() {
                            dc[i] = dc[i].wrapping_add(streams.dc(i)? as u8);
                            let (x, y) = p.macroblock(mx, my, sub);
                            let index = p.index(x, y);
                            p.blocks[index].dc = dc[i];
                        }
                    }
                } else {
                    dc = [127; 3];
                }
                let flags = (kind << 5) | (proc << 4);
                for (group, run) in runs.iter_mut().enumerate() {
                    for sub in 0..self.planes[group].count() {
                        let value = if proc == 1 {
                            0
                        } else {
                            streams.block_types(group == 1, run)?
                        };
                        if group == 0 && value > 15 {
                            return Err(Error::Invalid("luma block type"));
                        }
                        let (x, y) = self.planes[group].macroblock(mx, my, sub);
                        let index = self.planes[group].index(x, y);
                        self.planes[group].blocks[index].kind = flags | (value & 15);
                        if group == 1 {
                            self.planes[2].blocks[index].kind = flags | (value >> 4);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn inter(&mut self, streams: &mut Streams<'_>, frame_type: FrameType) -> Result<()> {
        self.descriptors(streams)?;
        let landscape = self.info.width >= self.info.height;
        let mut reference = 3;
        let mut motion = [0; 2];
        let [dst, past, future] = self
            .frames
            .get_disjoint_mut([self.current, self.past, self.future])
            .unwrap();
        let (dst, past, future) = (dst.as_mut_slice(), past.as_slice(), future.as_slice());
        for my in 0..usize::from(self.info.height) / 8 {
            for mx in 0..usize::from(self.info.width) / 8 {
                let flags = self.planes[0].blocks[self.planes[0].index(mx * 2, my * 2)].kind;
                let target = (flags >> 5) & 3;
                if target == 0 {
                    for (i, p) in self.planes.iter().enumerate() {
                        for sub in 0..p.count() {
                            let (x, y) = p.macroblock(mx, my, sub);
                            let index = p.index(x, y);
                            let block = p.blocks[index];
                            let pixels = if block.kind == 0 {
                                weighted(
                                    block.dc,
                                    [index - p.stride, index + p.stride, index - 1, index + 1]
                                        .map(|n| neighbor(&p.blocks, n, block.dc)),
                                )
                            } else {
                                intra_block(streams, i, block, &self.nest, landscape)?
                            };
                            p.put(dst, x, y, &pixels);
                        }
                    }
                    continue;
                }
                let selected = usize::from(target - 1);
                if selected != reference {
                    reference = selected;
                    motion = [0; 2];
                }
                for (axis, value) in motion.iter_mut().enumerate() {
                    streams.motion(axis, reference, value)?;
                }
                let rx = mx as i32 * 16 + motion[0];
                let ry = my as i32 * 16 + motion[1];
                let nest_origin = if landscape {
                    i64::from(rx / 2 - 32) + i64::from(ry / 2 - 16) * i64::from(self.info.width)
                } else {
                    i64::from(rx / 2 - 16) + i64::from(ry / 2 - 32) * i64::from(self.info.width)
                };
                for (i, p) in self.planes.iter().enumerate() {
                    let px = rx >> p.sx;
                    let py = ry >> p.sy;
                    let (hx, hy) = if self.info.version == Version::V15 {
                        ((px & 1) as usize, (py & 1) as usize)
                    } else {
                        ((rx & 1) as usize, (ry & 1) as usize)
                    };
                    for sub in 0..p.count() {
                        let (x, y) = p.macroblock(mx, my, sub);
                        let kind = p.blocks[p.index(x, y)].kind & 15;
                        let (bx, by) = p.macroblock(0, 0, sub);
                        let source_x = (px >> 1) + (bx * 4) as i32;
                        let source_y = (py >> 1) + (by * 4) as i32;
                        if kind == 0 && reference == 1 && frame_type == FrameType::P {
                            // Plain motion copies update pixels in raster order.
                            // Overlapping source regions must see earlier writes,
                            // including those within this same 4x4 block.
                            motion_block_in_place(dst, p, (x, y), (source_x, source_y), (hx, hy))?;
                            continue;
                        }
                        // The reference decoder passes the destination itself as
                        // the second reference for P pictures. Read it per block
                        // so previously reconstructed blocks remain visible.
                        let source: &[u8] = if reference == 0 {
                            past
                        } else if frame_type == FrameType::P {
                            dst
                        } else {
                            future
                        };
                        let pixels = if kind == 6 {
                            literal(streams, i)?
                        } else {
                            let predicted = motion_block(source, p, source_x, source_y, hx, hy)?;
                            if kind == 0 {
                                predicted
                            } else {
                                if kind > 5 {
                                    return Err(Error::Invalid("predictive block type"));
                                }
                                let nest = Nest {
                                    data: &source[..self.planes[0].width * self.planes[0].height],
                                    stride: self.planes[0].width,
                                    origin: nest_origin,
                                    landscape,
                                    quantize: true,
                                };
                                predicted_block(streams, i, kind, predicted, nest)?
                            }
                        };
                        p.put(dst, x, y, &pixels);
                    }
                }
            }
        }
        Ok(())
    }
}

fn neighbor(blocks: &[Block], index: usize, fallback: u8) -> u8 {
    let b = blocks[index];
    if b.kind & 0x77 != 0 {
        fallback
    } else {
        b.dc
    }
}

fn weighted(dc: u8, neighbors: [u8; 4]) -> [u8; 16] {
    // Edge weights are separable. Retain the reference's unsigned divide and
    // wrap before saturation, including its behavior on negative numerators.
    let [top, bottom, left, right] = neighbors.map(i32::from);
    let mut out = [0; 16];
    const WEIGHTS: [i32; 4] = [2, 0, -1, -1];
    for y in 0..4 {
        for x in 0..4 {
            let t = WEIGHTS[y];
            let b = WEIGHTS[3 - y];
            let l = WEIGHTS[x];
            let r = WEIGHTS[3 - x];
            let value =
                (8 - t - b - l - r) * i32::from(dc) + t * top + b * bottom + l * left + r * right;
            out[y * 4 + x] = ((value as u32).wrapping_add(4) / 8).min(255) as u8;
        }
    }
    out
}

fn literal(streams: &mut Streams<'_>, plane: usize) -> Result<[u8; 16]> {
    let mut pixels = [0; 16];
    pixels.copy_from_slice(streams.bits[6 + 3 * plane].bytes(16)?);
    Ok(pixels)
}

#[derive(Clone, Copy)]
struct Nest<'a> {
    data: &'a [u8],
    stride: usize,
    origin: i64,
    landscape: bool,
    quantize: bool,
}

fn aot(
    streams: &mut Streams<'_>,
    plane: usize,
    count: u8,
    nest: Nest<'_>,
) -> Result<([i32; 16], i32)> {
    let mut result = [0i32; 16];
    let mut coefficient = 0i32;
    for _ in 0..count {
        let bytes = streams.bits[6 + 3 * plane].bytes(2)?;
        let code = u16::from_be_bytes([bytes[0], bytes[1]]);
        let big = usize::from(code & 63);
        let small = usize::from((code >> 6) & 31);
        let big_step = 1 << ((code >> 11) & 1);
        let small_step = 1 << ((code >> 12) & 1);
        let (x, y, dx, dy) = if nest.landscape {
            (big, small, big_step, small_step)
        } else {
            (small, big, small_step, big_step)
        };
        let mut basis = [0u8; 16];
        let mut lo = 255;
        let mut hi = 0;
        for by in 0..4 {
            for bx in 0..4 {
                let offset = nest.origin + ((y + by * dy) * nest.stride + x + bx * dx) as i64;
                let value = *nest
                    .data
                    .get(
                        usize::try_from(offset)
                            .map_err(|_| Error::Invalid("transform nest outside reference"))?,
                    )
                    .ok_or(Error::Invalid("transform nest outside reference"))?;
                let value = if nest.quantize { value >> 4 } else { value };
                basis[by * 4 + bx] = value;
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
        coefficient = coefficient.wrapping_add(streams.huff(5 + 3 * plane)?);
        let range = i32::from(hi - lo);
        let inverse = if range == 0 {
            0
        } else {
            (4096 / (range * 16)) * 16
        };
        let inverse = if code & 0x8000 != 0 {
            -inverse
        } else {
            inverse
        };
        let factor = coefficient
            .wrapping_add(i32::from((code >> 13) & 3))
            .wrapping_mul(inverse);
        for (out, value) in result.iter_mut().zip(basis) {
            *out = out.wrapping_add(factor.wrapping_mul(i32::from(value)));
        }
    }
    let mean = result.iter().fold(0i32, |a, &b| a.wrapping_add(b)) >> 4;
    Ok((result, mean))
}

fn intra_block(
    streams: &mut Streams<'_>,
    plane: usize,
    block: Block,
    nest: &[u8],
    landscape: bool,
) -> Result<[u8; 16]> {
    match block.kind {
        6 => literal(streams, plane),
        8 => Ok([block.dc; 16]),
        1..=5 => {
            let nest = Nest {
                data: nest,
                stride: if landscape { 70 } else { 38 },
                origin: 0,
                landscape,
                quantize: false,
            };
            let (result, mean) = aot(streams, plane, block.kind, nest)?;
            let delta = i32::from(block.dc)
                .wrapping_shl(u32::from(streams.transform_shift))
                .wrapping_sub(mean);
            Ok(result
                .map(|v| (v.wrapping_add(delta) >> streams.transform_shift).clamp(0, 255) as u8))
        }
        _ => Err(Error::Invalid("intra block type")),
    }
}

fn motion_block(
    source: &[u8],
    p: &Plane,
    x: i32,
    y: i32,
    hx: usize,
    hy: usize,
) -> Result<[u8; 16]> {
    let start = motion_origin(p, x, y, hx, hy)?;
    let mut out = [0; 16];
    for row in 0..4 {
        let pos = start + row * p.width;
        if hx == 0 && hy == 0 {
            out[row * 4..row * 4 + 4].copy_from_slice(&source[pos..pos + 4]);
        } else {
            for col in 0..4 {
                let at = pos + col;
                let sum = u16::from(source[at])
                    + u16::from(source[at + hx])
                    + u16::from(source[at + hy * p.width])
                    + u16::from(source[at + hy * p.width + hx]);
                out[row * 4 + col] = ((sum + 2) >> 2) as u8;
            }
        }
    }
    Ok(out)
}

fn motion_origin(p: &Plane, x: i32, y: i32, hx: usize, hy: usize) -> Result<usize> {
    if x < 0 || y < 0 || x as usize + 4 + hx > p.width || y as usize + 4 + hy > p.height {
        return Err(Error::Invalid("motion vector outside reference plane"));
    }
    Ok(p.offset + y as usize * p.width + x as usize)
}

fn motion_block_in_place(
    dst: &mut [u8],
    p: &Plane,
    block: (usize, usize),
    source: (i32, i32),
    half: (usize, usize),
) -> Result<()> {
    let start = motion_origin(p, source.0, source.1, half.0, half.1)?;
    let target = p.offset + block.1 * 4 * p.width + block.0 * 4;
    for row in 0..4 {
        for col in 0..4 {
            let at = start + row * p.width + col;
            let sum = u16::from(dst[at])
                + u16::from(dst[at + half.0])
                + u16::from(dst[at + half.1 * p.width])
                + u16::from(dst[at + half.1 * p.width + half.0]);
            dst[target + row * p.width + col] = ((sum + 2) >> 2) as u8;
        }
    }
    Ok(())
}

fn predicted_block(
    streams: &mut Streams<'_>,
    plane: usize,
    kind: u8,
    predicted: [u8; 16],
    nest: Nest<'_>,
) -> Result<[u8; 16]> {
    let (result, average) = aot(streams, plane, kind - 1, nest)?;
    let mean = (predicted.iter().map(|&v| i32::from(v)).sum::<i32>() + 8) / 16;
    let lo = *predicted.iter().min().unwrap();
    let hi = *predicted.iter().max().unwrap();
    let delta = (streams.dc(plane)? >> streams.dc_shift)
        .wrapping_shl(u32::from(streams.transform_shift))
        .wrapping_sub(average);
    let inverse = if hi == lo {
        0
    } else {
        4096 / i32::from(hi - lo)
    };
    let factor = (streams.dc(plane)? >> streams.dc_shift).wrapping_mul(inverse);
    Ok(std::array::from_fn(|i| {
        let value = result[i]
            .wrapping_add(delta)
            .wrapping_add((i32::from(predicted[i]) - mean).wrapping_mul(factor));
        (value >> streams.transform_shift)
            .wrapping_add(i32::from(predicted[i]))
            .clamp(0, 255) as u8
    }))
}
