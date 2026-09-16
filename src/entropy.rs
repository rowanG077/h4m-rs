use crate::error::{be32, Error, Result};

#[derive(Clone, Copy, Default)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub fn bit(&mut self) -> Result<u32> {
        let byte = *self.data.get(self.pos >> 3).ok_or(Error::Truncated)?;
        let value = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(u32::from(value))
    }

    pub fn bits(&mut self, n: u8) -> Result<u32> {
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let start = self.pos >> 3;
        let result = self.data.get(start..start + n).ok_or(Error::Truncated)?;
        self.pos += n * 8;
        Ok(result)
    }

    #[inline]
    fn peek8(&self) -> Option<usize> {
        if self.data.len() * 8 - self.pos < 8 {
            return None;
        }
        let p = self.pos >> 3;
        let shift = self.pos & 7;
        let word = (u16::from(self.data[p]) << 8) | u16::from(*self.data.get(p + 1).unwrap_or(&0));
        Some(((word >> (8 - shift)) & 255) as usize)
    }
}

#[derive(Clone, Copy, Default)]
struct Lookup {
    node: u16,
    bits: u8,
}

struct Tree {
    children: [[u16; 2]; 511],
    values: [i32; 256],
    root: u16,
    lookup: [Lookup; 256],
}

impl Tree {
    fn new() -> Self {
        Self {
            children: [[0; 2]; 511],
            values: [0; 256],
            root: 0,
            lookup: [Lookup::default(); 256],
        }
    }

    fn read(&mut self, bits: &mut Bits<'_>, signed: bool, shift: u8) -> Result<()> {
        if !bits.data.is_empty() {
            self.root = self.node(bits, signed, shift, &mut 256)?;
        }
        for (prefix, entry) in self.lookup.iter_mut().enumerate() {
            let mut node = self.root;
            let mut count = 0;
            while node >= 256 && count < 8 {
                node = self.children[node as usize][(prefix >> (7 - count)) & 1];
                count += 1;
            }
            *entry = Lookup { node, bits: count };
        }
        Ok(())
    }

    fn node(
        &mut self,
        bits: &mut Bits<'_>,
        signed: bool,
        shift: u8,
        next: &mut u16,
    ) -> Result<u16> {
        if bits.bit()? == 0 {
            let byte = bits.bits(8)? as u8;
            self.values[byte as usize] = if signed {
                i32::from(byte as i8)
            } else {
                i32::from(byte)
            } << shift;
            Ok(u16::from(byte))
        } else {
            if *next >= 511 {
                return Err(Error::Invalid("Huffman tree has too many branches"));
            }
            let node = *next;
            *next += 1;
            let left = self.node(bits, signed, shift, next)?;
            let right = self.node(bits, signed, shift, next)?;
            self.children[node as usize] = [left, right];
            Ok(node)
        }
    }

    #[inline]
    fn symbol(&self, bits: &mut Bits<'_>) -> Result<i32> {
        let mut node = self.root;
        if node >= 256 {
            if let Some(prefix) = bits.peek8() {
                let entry = self.lookup[prefix];
                bits.pos += usize::from(entry.bits);
                node = entry.node;
            }
            while node >= 256 {
                node = self.children[node as usize][bits.bit()? as usize];
            }
        }
        Ok(self.values[node as usize])
    }
}

// Stream indices follow the packet's offset table. Trees are shared across planes.
pub(crate) struct Streams<'a> {
    pub bits: [Bits<'a>; 17],
    trees: [Tree; 6],
    inter: bool,
    pub dc_shift: u8,
    pub transform_shift: u8,
    pub residual: [[u8; 2]; 2],
}

impl<'a> Streams<'a> {
    pub fn new(packet: &'a [u8], inter: bool) -> Result<Self> {
        let count = if inter { 17 } else { 16 };
        let base = 8 + 4 * count;
        if packet.len() < base {
            return Err(Error::Truncated);
        }
        if packet[0] > 8 || packet[1] > 30 {
            return Err(Error::Invalid("coefficient shift"));
        }
        let mut result = Self {
            bits: [Bits::default(); 17],
            trees: std::array::from_fn(|_| Tree::new()),
            inter,
            dc_shift: packet[0],
            transform_shift: packet[1],
            residual: [[packet[2], packet[3]], [packet[4], packet[5]]],
        };
        if inter && packet[2..6].iter().any(|&v| v > 8) {
            return Err(Error::Invalid("motion residual width"));
        }
        for i in 0..count {
            let offset = base
                .checked_add(be32(packet, 8 + i * 4)? as usize)
                .ok_or(Error::Truncated)?;
            let size = be32(packet, offset)? as usize;
            let start = offset.checked_add(4).ok_or(Error::Truncated)?;
            let end = start.checked_add(size).ok_or(Error::Truncated)?;
            result.bits[i] = Bits::new(packet.get(start..end).ok_or(Error::Truncated)?);
        }
        for (tree, stream, signed, shift) in [
            (3, 0, false, 0),
            (1, 1, false, 0),
            (0, 4, true, packet[0]),
            (2, 5, false, 2),
        ] {
            result.trees[tree].read(&mut result.bits[stream], signed, shift)?;
        }
        if inter {
            result.trees[4].read(&mut result.bits[13], true, 0)?;
            result.trees[5].read(&mut result.bits[15], false, 0)?;
        }
        Ok(result)
    }

    #[inline]
    pub fn huff(&mut self, stream: usize) -> Result<i32> {
        let tree = match stream {
            0 | 2 => 3,
            1 | 3 => 1,
            4 | 7 | 10 => 0,
            5 | 8 | 11 => 2,
            13 | 14 if self.inter => 4,
            15 | 16 if self.inter => 5,
            13..=15 => 1,
            _ => unreachable!(),
        };
        self.trees[tree].symbol(&mut self.bits[stream])
    }

    pub fn dc(&mut self, plane: usize) -> Result<i32> {
        let stream = 4 + plane * 3;
        let min = -128 << self.dc_shift;
        let max = 127 << self.dc_shift;
        self.overflow(stream, min, max)
    }

    pub fn run(&mut self, stream: usize) -> Result<u32> {
        Ok(self.overflow(stream, -1, 255)? as u32)
    }

    fn overflow(&mut self, stream: usize, min: i32, max: i32) -> Result<i32> {
        let mut sum: i32 = 0;
        loop {
            let before = self.bits[stream].pos;
            let value = self.huff(stream)?;
            sum = sum
                .checked_add(value)
                .ok_or(Error::Invalid("coefficient overflow"))?;
            if value > min && value < max {
                return Ok(sum);
            }
            if self.bits[stream].pos == before {
                return Err(Error::Invalid("nonterminating Huffman escape"));
            }
        }
    }

    pub fn delta(&mut self, plane: usize, run: &mut u32) -> Result<i32> {
        if *run > 0 {
            *run -= 1;
            return Ok(0);
        }
        let delta = self.dc(plane)?;
        if delta == 0 {
            *run = self.huff(13 + plane)? as u32;
        }
        Ok(delta)
    }

    pub fn block_types(&mut self, chroma: bool, run: &mut u32) -> Result<u8> {
        if *run > 0 {
            *run -= 1;
            return Ok(0);
        }
        let stream = if chroma { 2 } else { 0 };
        let value = self.huff(stream)? as u8;
        if value == 0 {
            *run = self.huff(stream + 1)? as u32;
        }
        Ok(value)
    }

    pub fn motion(&mut self, axis: usize, reference: usize, value: &mut i32) -> Result<()> {
        let bits = self.residual[reference][axis];
        let delta = (self.huff(13 + axis)? << bits) + self.bits[13 + axis].bits(bits)? as i32;
        let limit = 1 << (bits + 5);
        let next = *value + delta;
        if !(-3 * limit..3 * limit).contains(&next) {
            return Err(Error::Invalid("motion vector range"));
        }
        // Wrap once into the signed, power-of-two motion range.
        *value = ((next + limit) & (2 * limit - 1)) - limit;
        Ok(())
    }
}

pub(crate) struct Runs {
    value: u8,
    count: u32,
    stream: usize,
}

impl Runs {
    pub fn new(streams: &mut Streams<'_>, stream: usize, n: u8) -> Result<Self> {
        // An absent proc stream is valid when every macroblock is intra.
        if streams.bits[stream].data.is_empty() {
            return Ok(Self {
                value: 0,
                count: 0,
                stream,
            });
        }
        let value = streams.bits[stream].bits(n)? as u8;
        if value > 2 {
            return Err(Error::Invalid("macroblock reference"));
        }
        Ok(Self {
            value,
            count: streams.run(stream)?,
            stream,
        })
    }

    pub fn next(&mut self, streams: &mut Streams<'_>) -> Result<u8> {
        if self.count == 0 {
            if self.stream == 15 {
                self.value = (self.value + 1 + streams.bits[15].bit()? as u8) % 3;
            } else {
                self.value ^= 1;
            }
            self.count = streams.run(self.stream)?;
        }
        if self.count == 0 {
            return Err(Error::Invalid("empty macroblock run"));
        }
        self.count -= 1;
        Ok(self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(bits: &[bool]) -> Vec<u8> {
        let mut bytes = vec![0; bits.len().div_ceil(8)];
        for (i, &bit) in bits.iter().enumerate() {
            bytes[i / 8] |= u8::from(bit) << (7 - i % 8);
        }
        bytes
    }

    fn leaf(bits: &mut Vec<bool>, value: u8) {
        bits.push(false);
        for shift in (0..8).rev() {
            bits.push(value & (1 << shift) != 0);
        }
    }

    #[test]
    fn long_codes_cross_lookup_and_byte_boundaries() {
        // A maximally skewed, 256-leaf tree, with codes up to 255 bits long.
        let mut bits = Vec::new();
        for value in 0..255 {
            bits.push(true);
            leaf(&mut bits, value);
        }
        leaf(&mut bits, 255);
        let expected = [0u8, 1, 127, 254, 255, 0, 200];
        for value in expected {
            bits.extend(std::iter::repeat_n(true, value as usize));
            if value != 255 {
                bits.push(false);
            }
        }
        let bytes = pack(&bits);
        let mut input = Bits::new(&bytes);
        let mut tree = Tree::new();
        tree.read(&mut input, true, 2).unwrap();
        for value in expected {
            assert_eq!(tree.symbol(&mut input).unwrap(), i32::from(value as i8) * 4);
        }
    }

    #[test]
    fn trees_are_bounded_and_truncated_codes_fail() {
        let mut tree = Tree::new();
        assert!(matches!(
            tree.read(&mut Bits::new(&[255; 40]), false, 0),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            tree.read(&mut Bits::new(&[0]), false, 0),
            Err(Error::Truncated)
        ));
        let mut bits = vec![true];
        leaf(&mut bits, 0);
        leaf(&mut bits, 1);
        let bytes = pack(&bits);
        tree.read(&mut Bits::new(&bytes), false, 0).unwrap();
        assert!(matches!(
            tree.symbol(&mut Bits::default()),
            Err(Error::Truncated)
        ));
    }

    #[test]
    fn constant_escape_is_rejected_without_looping() {
        let mut streams = Streams {
            bits: [Bits::default(); 17],
            trees: std::array::from_fn(|_| Tree::new()),
            inter: false,
            dc_shift: 0,
            transform_shift: 12,
            residual: [[0; 2]; 2],
        };
        streams.trees[0].values[0] = 127;
        assert!(matches!(
            streams.dc(0),
            Err(Error::Invalid("nonterminating Huffman escape"))
        ));
    }

    #[test]
    fn motion_vectors_stay_within_the_coded_range() {
        let mut packet = [0; 80];
        packet[2..6].fill(8);
        let mut streams = Streams::new(&packet, true).unwrap();
        for (symbol, mut value, expected) in [(31, 7936, -512), (-32, -8192, 0)] {
            streams.trees[4].values[0] = symbol;
            streams.bits[13] = Bits::new(&[0]);
            streams.motion(0, 0, &mut value).unwrap();
            assert_eq!(value, expected);
        }
        for symbol in [-128, 127] {
            streams.trees[4].values[0] = symbol;
            streams.bits[13] = Bits::new(&[0]);
            assert!(matches!(
                streams.motion(0, 0, &mut 0),
                Err(Error::Invalid(_))
            ));
        }
    }
}
