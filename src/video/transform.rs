//! Adaptive orthogonal transforms and intra/predicted 4×4 reconstruction.
use crate::{
    entropy::Streams,
    error::{Error, Result},
    syntax::{IntraBlock, MotionVector, Orientation, PlaneId},
};

pub(super) fn weighted(dc: u8, neighbors: [u8; 4]) -> [u8; 16] {
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

#[derive(Clone, Copy)]
pub(super) struct Nest<'a> {
    data: &'a [u8],
    stride: usize,
    origin: i64,
    orientation: Orientation,
    source: NestSource,
}

#[derive(Clone, Copy)]
enum NestSource {
    Dc,
    ReferencePixels,
}

impl<'a> Nest<'a> {
    pub fn dc(data: &'a [u8], orientation: Orientation) -> Self {
        Self {
            data,
            stride: orientation.nest_dimensions().0,
            origin: 0,
            orientation,
            source: NestSource::Dc,
        }
    }
    pub fn reference(
        data: &'a [u8],
        stride: usize,
        position: MotionVector,
        orientation: Orientation,
    ) -> Self {
        let (left, top) = match orientation {
            Orientation::Landscape => (32, 16),
            Orientation::Portrait => (16, 32),
        };
        let origin =
            i64::from(position.x / 2 - left) + i64::from(position.y / 2 - top) * stride as i64;
        Self {
            data,
            stride,
            origin,
            orientation,
            source: NestSource::ReferencePixels,
        }
    }
}

/// The compact two-byte basis descriptor is decoded once at the boundary.
struct BasisDescriptor {
    x: usize,
    y: usize,
    horizontal_step: usize,
    vertical_step: usize,
    coefficient_low: u8,
    negative: bool,
}
impl BasisDescriptor {
    fn parse(code: u16, orientation: Orientation) -> Self {
        let long_position = usize::from(code & 0x3f);
        let short_position = usize::from((code >> 6) & 0x1f);
        let long_step = 1 << ((code >> 11) & 1);
        let short_step = 1 << ((code >> 12) & 1);
        let (x, y, horizontal_step, vertical_step) = match orientation {
            Orientation::Landscape => (long_position, short_position, long_step, short_step),
            Orientation::Portrait => (short_position, long_position, short_step, long_step),
        };
        Self {
            x,
            y,
            horizontal_step,
            vertical_step,
            coefficient_low: ((code >> 13) & 3) as u8,
            negative: code & 0x8000 != 0,
        }
    }
}

fn aot(
    streams: &mut Streams<'_>,
    plane: PlaneId,
    count: u8,
    nest: Nest<'_>,
) -> Result<([i32; 16], i32)> {
    let mut result = [0i32; 16];
    let mut coefficient = 0i32;
    for _ in 0..count {
        let descriptor = BasisDescriptor::parse(streams.basis_descriptor(plane)?, nest.orientation);
        let mut basis = [0u8; 16];
        let mut lo = 255;
        let mut hi = 0;
        for by in 0..4 {
            for bx in 0..4 {
                let offset = nest.origin
                    + ((descriptor.y + by * descriptor.vertical_step) * nest.stride
                        + descriptor.x
                        + bx * descriptor.horizontal_step) as i64;
                let value = *nest
                    .data
                    .get(
                        usize::try_from(offset)
                            .map_err(|_| Error::Invalid("transform nest outside reference"))?,
                    )
                    .ok_or(Error::Invalid("transform nest outside reference"))?;
                let value = if matches!(nest.source, NestSource::ReferencePixels) {
                    value >> 4
                } else {
                    value
                };
                basis[by * 4 + bx] = value;
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
        coefficient = coefficient.wrapping_add(streams.coefficient(plane)?);
        let range = i32::from(hi - lo);
        let inverse = if range == 0 {
            0
        } else {
            (4096 / (range * 16)) * 16
        };
        let inverse = if descriptor.negative {
            -inverse
        } else {
            inverse
        };
        let factor = coefficient
            .wrapping_add(i32::from(descriptor.coefficient_low))
            .wrapping_mul(inverse);
        for (out, value) in result.iter_mut().zip(basis) {
            *out = out.wrapping_add(factor.wrapping_mul(i32::from(value)));
        }
    }
    let mean = result.iter().fold(0i32, |a, &b| a.wrapping_add(b)) >> 4;
    Ok((result, mean))
}

pub(super) fn intra_block(
    streams: &mut Streams<'_>,
    plane: PlaneId,
    coding: IntraBlock,
    dc: u8,
    nest: Nest<'_>,
) -> Result<[u8; 16]> {
    match coding {
        IntraBlock::Literal => streams.literal(plane),
        IntraBlock::Solid => Ok([dc; 16]),
        IntraBlock::Transform(bases) => {
            let (result, mean) = aot(streams, plane, bases, nest)?;
            let delta = i32::from(dc)
                .wrapping_shl(u32::from(streams.transform_shift))
                .wrapping_sub(mean);
            Ok(result.map(|value| {
                (value.wrapping_add(delta) >> streams.transform_shift).clamp(0, 255) as u8
            }))
        }
        IntraBlock::Weighted => Err(Error::Invalid("weighted block requires neighbors")),
    }
}

pub(super) fn predicted_block(
    streams: &mut Streams<'_>,
    plane: PlaneId,
    basis_count: u8,
    predicted: [u8; 16],
    nest: Nest<'_>,
) -> Result<[u8; 16]> {
    let (result, average) = aot(streams, plane, basis_count, nest)?;
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
    Ok(core::array::from_fn(|i| {
        let value = result[i]
            .wrapping_add(delta)
            .wrapping_add((i32::from(predicted[i]) - mean).wrapping_mul(factor));
        (value >> streams.transform_shift)
            .wrapping_add(i32::from(predicted[i]))
            .clamp(0, 255) as u8
    }))
}
