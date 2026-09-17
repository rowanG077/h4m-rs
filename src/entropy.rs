//! Bounded bit readers, Huffman codebooks, and named picture entropy streams.

use crate::{
    error::{be16, be32, Error, Result},
    syntax::{
        MacroblockMode, MotionMode, MotionVector, PlaneGroup, PlaneId, Planes, PredictionTarget,
        Reference,
    },
    FrameType,
};

#[derive(Clone, Copy, Default)]
struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    #[inline]
    fn bit(&mut self) -> Result<bool> {
        let byte = *self.data.get(self.position / 8).ok_or(Error::Truncated)?;
        let value = byte & (1 << (7 - self.position % 8)) != 0;
        self.position += 1;
        Ok(value)
    }

    fn bits(&mut self, count: u8) -> Result<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.bit()?);
        }
        Ok(value)
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let start = self.position / 8;
        let end = start.checked_add(N).ok_or(Error::Truncated)?;
        let data = self.data.get(start..end).ok_or(Error::Truncated)?;
        let mut result = [0; N];
        result.copy_from_slice(data);
        self.position += N * 8;
        Ok(result)
    }

    #[inline]
    fn peek_byte(&self) -> Option<usize> {
        if self.data.len() * 8 - self.position < 8 {
            return None;
        }
        let index = self.position / 8;
        let shift = self.position % 8;
        let word =
            (u16::from(self.data[index]) << 8) | u16::from(*self.data.get(index + 1).unwrap_or(&0));
        Some(((word >> (8 - shift)) & 255) as usize)
    }
}

#[derive(Clone, Copy)]
enum Node {
    Leaf(u8),
    Branch(u8),
}

#[derive(Clone, Copy)]
struct Lookup {
    node: Node,
    consumed: u8,
}

#[derive(Clone, Copy)]
enum SymbolKind {
    Unsigned,
    Signed,
}

struct Tree {
    branches: [[Node; 2]; 255],
    values: [i32; 256],
    root: Node,
    lookup: [Lookup; 256],
}

impl Tree {
    fn new() -> Self {
        Self {
            branches: [[Node::Leaf(0); 2]; 255],
            values: [0; 256],
            root: Node::Leaf(0),
            lookup: [Lookup {
                node: Node::Leaf(0),
                consumed: 0,
            }; 256],
        }
    }

    fn read(&mut self, bits: &mut Bits<'_>, kind: SymbolKind, shift: u8) -> Result<()> {
        if !bits.data.is_empty() {
            self.root = self.read_node(bits, kind, shift, &mut 0)?;
        }
        for (prefix, entry) in self.lookup.iter_mut().enumerate() {
            let mut node = self.root;
            let mut consumed = 0;
            while let Node::Branch(index) = node {
                if consumed == 8 {
                    break;
                }
                node = self.branches[usize::from(index)][(prefix >> (7 - consumed)) & 1];
                consumed += 1;
            }
            *entry = Lookup { node, consumed };
        }
        Ok(())
    }

    fn read_node(
        &mut self,
        bits: &mut Bits<'_>,
        kind: SymbolKind,
        shift: u8,
        next: &mut usize,
    ) -> Result<Node> {
        if !bits.bit()? {
            let byte = bits.bits(8)? as u8;
            self.values[usize::from(byte)] = match kind {
                SymbolKind::Unsigned => i32::from(byte),
                SymbolKind::Signed => i32::from(byte as i8),
            } << shift;
            Ok(Node::Leaf(byte))
        } else {
            if *next == self.branches.len() {
                return Err(Error::Invalid("Huffman tree has too many branches"));
            }
            let index = *next;
            *next += 1;
            let left = self.read_node(bits, kind, shift, next)?;
            let right = self.read_node(bits, kind, shift, next)?;
            self.branches[index] = [left, right];
            Ok(Node::Branch(index as u8))
        }
    }

    #[inline]
    fn symbol(&self, bits: &mut Bits<'_>) -> Result<i32> {
        let mut node = self.root;
        if matches!(node, Node::Branch(_)) {
            if let Some(prefix) = bits.peek_byte() {
                let entry = self.lookup[prefix];
                bits.position += usize::from(entry.consumed);
                node = entry.node;
            }
        }
        loop {
            match node {
                Node::Leaf(byte) => return Ok(self.values[usize::from(byte)]),
                Node::Branch(index) => {
                    node = self.branches[usize::from(index)][usize::from(bits.bit()?)]
                }
            }
        }
    }

    fn escaped(&self, bits: &mut Bits<'_>, min: i32, max: i32) -> Result<i32> {
        let mut sum = 0i32;
        loop {
            let before = bits.position;
            let value = self.symbol(bits)?;
            sum = sum
                .checked_add(value)
                .ok_or(Error::Invalid("coefficient overflow"))?;
            if value > min && value < max {
                return Ok(sum);
            }
            if bits.position == before {
                return Err(Error::Invalid("nonterminating Huffman escape"));
            }
        }
    }
}

struct Codebooks {
    block: Tree,
    zero_run: Tree,
    dc: Tree,
    coefficient: Tree,
    motion: Tree,
    macroblock_run: Tree,
}

impl Codebooks {
    fn new() -> Self {
        Self {
            block: Tree::new(),
            zero_run: Tree::new(),
            dc: Tree::new(),
            coefficient: Tree::new(),
            motion: Tree::new(),
            macroblock_run: Tree::new(),
        }
    }
}

struct BlockStreams<'a> {
    kinds: Bits<'a>,
    zero_runs: Bits<'a>,
}

struct PlaneStreams<'a> {
    dc: Bits<'a>,
    coefficients: Bits<'a>,
    details: Bits<'a>,
}

struct MotionWidths {
    horizontal: u8,
    vertical: u8,
}

struct InterStreams<'a> {
    horizontal: Bits<'a>,
    vertical: Bits<'a>,
    past_widths: MotionWidths,
    second_widths: MotionWidths,
    targets: RunStream<'a, PredictionTarget>,
    modes: RunStream<'a, MotionMode>,
}

enum PictureStreams<'a> {
    Intra {
        zero_runs: Planes<Bits<'a>>,
        nest_x: usize,
        nest_y: usize,
    },
    Inter(InterStreams<'a>),
}

pub(crate) struct Streams<'a> {
    luma: BlockStreams<'a>,
    chroma: BlockStreams<'a>,
    planes: Planes<PlaneStreams<'a>>,
    picture: PictureStreams<'a>,
    codebooks: Codebooks,
    pub dc_shift: u8,
    pub transform_shift: u8,
}

impl<'a> Streams<'a> {
    pub fn new(packet: &'a [u8], kind: FrameType) -> Result<Self> {
        let count = if kind == FrameType::I { 16 } else { 17 };
        let base = 8 + 4 * count;
        if packet.len() < base {
            return Err(Error::Truncated);
        }
        if packet[0] > 8 || packet[1] > 30 {
            return Err(Error::Invalid("coefficient shift"));
        }
        // The sole positional mapping: the format's on-wire offset table.
        // All decoding after this boundary uses named, typed streams.
        let mut raw = [Bits::default(); 17];
        for (index, stream) in raw.iter_mut().take(count).enumerate() {
            let offset = base
                .checked_add(be32(packet, 8 + index * 4)? as usize)
                .ok_or(Error::Truncated)?;
            let size = be32(packet, offset)? as usize;
            let start = offset.checked_add(4).ok_or(Error::Truncated)?;
            let end = start.checked_add(size).ok_or(Error::Truncated)?;
            *stream = Bits::new(packet.get(start..end).ok_or(Error::Truncated)?);
        }
        let [mut luma_types, mut luma_runs, chroma_types, chroma_runs, mut y_dc, mut y_coefficients, y_details, u_dc, u_coefficients, u_details, v_dc, v_coefficients, v_details, mut extra_x, extra_y, mut extra_types, extra_modes] =
            raw;
        let mut codebooks = Codebooks::new();
        codebooks
            .block
            .read(&mut luma_types, SymbolKind::Unsigned, 0)?;
        codebooks
            .zero_run
            .read(&mut luma_runs, SymbolKind::Unsigned, 0)?;
        codebooks
            .dc
            .read(&mut y_dc, SymbolKind::Signed, packet[0])?;
        codebooks
            .coefficient
            .read(&mut y_coefficients, SymbolKind::Unsigned, 2)?;
        let picture = match kind {
            FrameType::I => PictureStreams::Intra {
                zero_runs: Planes {
                    y: extra_x,
                    u: extra_y,
                    v: extra_types,
                },
                nest_x: usize::from(be16(packet, 4)?),
                nest_y: usize::from(be16(packet, 6)?),
            },
            FrameType::P | FrameType::B => {
                if packet[2..6].iter().any(|&width| width > 8) {
                    return Err(Error::Invalid("motion residual width"));
                }
                codebooks.motion.read(&mut extra_x, SymbolKind::Signed, 0)?;
                codebooks
                    .macroblock_run
                    .read(&mut extra_types, SymbolKind::Unsigned, 0)?;
                PictureStreams::Inter(InterStreams {
                    horizontal: extra_x,
                    vertical: extra_y,
                    past_widths: MotionWidths {
                        horizontal: packet[2],
                        vertical: packet[3],
                    },
                    second_widths: MotionWidths {
                        horizontal: packet[4],
                        vertical: packet[5],
                    },
                    targets: RunStream::new(extra_types, &codebooks.macroblock_run)?,
                    modes: RunStream::new(extra_modes, &codebooks.macroblock_run)?,
                })
            }
        };
        Ok(Self {
            luma: BlockStreams {
                kinds: luma_types,
                zero_runs: luma_runs,
            },
            chroma: BlockStreams {
                kinds: chroma_types,
                zero_runs: chroma_runs,
            },
            planes: Planes {
                y: PlaneStreams {
                    dc: y_dc,
                    coefficients: y_coefficients,
                    details: y_details,
                },
                u: PlaneStreams {
                    dc: u_dc,
                    coefficients: u_coefficients,
                    details: u_details,
                },
                v: PlaneStreams {
                    dc: v_dc,
                    coefficients: v_coefficients,
                    details: v_details,
                },
            },
            picture,
            codebooks,
            dc_shift: packet[0],
            transform_shift: packet[1],
        })
    }

    pub fn nest_origin(&self) -> Result<(usize, usize)> {
        match self.picture {
            PictureStreams::Intra { nest_x, nest_y, .. } => Ok((nest_x, nest_y)),
            _ => Err(Error::Invalid("DC nest in an inter picture")),
        }
    }

    pub fn dc(&mut self, plane: PlaneId) -> Result<i32> {
        self.codebooks.dc.escaped(
            &mut self.planes[plane].dc,
            -128 << self.dc_shift,
            127 << self.dc_shift,
        )
    }

    pub fn delta(&mut self, plane: PlaneId, remaining: &mut u32) -> Result<i32> {
        if *remaining > 0 {
            *remaining -= 1;
            return Ok(0);
        }
        let delta = self.dc(plane)?;
        if delta == 0 {
            let PictureStreams::Intra { zero_runs, .. } = &mut self.picture else {
                return Err(Error::Invalid("DC run in an inter picture"));
            };
            *remaining = self.codebooks.zero_run.symbol(&mut zero_runs[plane])? as u32;
        }
        Ok(delta)
    }

    pub fn block_types(&mut self, group: PlaneGroup, remaining: &mut u32) -> Result<u8> {
        if *remaining > 0 {
            *remaining -= 1;
            return Ok(0);
        }
        let streams = match group {
            PlaneGroup::Luma => &mut self.luma,
            PlaneGroup::Chroma => &mut self.chroma,
        };
        let value = self.codebooks.block.symbol(&mut streams.kinds)? as u8;
        if value == 0 {
            *remaining = self.codebooks.zero_run.symbol(&mut streams.zero_runs)? as u32;
        }
        Ok(value)
    }

    pub fn literal(&mut self, plane: PlaneId) -> Result<[u8; 16]> {
        self.planes[plane].details.bytes()
    }

    pub fn basis_descriptor(&mut self, plane: PlaneId) -> Result<u16> {
        Ok(u16::from_be_bytes(self.planes[plane].details.bytes()?))
    }

    pub fn coefficient(&mut self, plane: PlaneId) -> Result<i32> {
        self.codebooks
            .coefficient
            .symbol(&mut self.planes[plane].coefficients)
    }

    pub fn macroblock_mode(&mut self) -> Result<MacroblockMode> {
        let PictureStreams::Inter(streams) = &mut self.picture else {
            return Err(Error::Invalid("macroblock run in an intra picture"));
        };
        Ok(
            match streams
                .targets
                .next(&self.codebooks.macroblock_run)?
                .reference()
            {
                None => MacroblockMode::Intra,
                Some(reference) => MacroblockMode::Predicted {
                    reference,
                    motion: streams.modes.next(&self.codebooks.macroblock_run)?,
                },
            },
        )
    }

    pub fn motion(&mut self, reference: Reference, vector: &mut MotionVector) -> Result<()> {
        let PictureStreams::Inter(streams) = &mut self.picture else {
            return Err(Error::Invalid("motion in an intra picture"));
        };
        let widths = match reference {
            Reference::Past => &streams.past_widths,
            Reference::Second => &streams.second_widths,
        };
        advance_motion(
            &self.codebooks.motion,
            &mut streams.horizontal,
            widths.horizontal,
            &mut vector.x,
        )?;
        advance_motion(
            &self.codebooks.motion,
            &mut streams.vertical,
            widths.vertical,
            &mut vector.y,
        )
    }
}

fn advance_motion(tree: &Tree, input: &mut Bits<'_>, width: u8, value: &mut i32) -> Result<()> {
    let delta = (tree.symbol(input)? << width) + input.bits(width)? as i32;
    let limit = 1 << (width + 5);
    let next = *value + delta;
    if !(-3 * limit..3 * limit).contains(&next) {
        return Err(Error::Invalid("motion vector range"));
    }
    *value = ((next + limit) & (2 * limit - 1)) - limit;
    Ok(())
}

/// A run holds a typed symbol. Absence is valid until that stream is consumed.
struct RunStream<'a, T> {
    input: Bits<'a>,
    value: Option<T>,
    remaining: u32,
}

trait RunSymbol: Copy {
    fn initial(input: &mut Bits<'_>) -> Result<Self>;
    fn next(self, input: &mut Bits<'_>) -> Result<Self>;
}

impl RunSymbol for PredictionTarget {
    fn initial(input: &mut Bits<'_>) -> Result<Self> {
        (input.bits(2)? as u8).try_into()
    }

    fn next(self, input: &mut Bits<'_>) -> Result<Self> {
        Ok(self.transition(input.bit()?))
    }
}

impl RunSymbol for MotionMode {
    fn initial(input: &mut Bits<'_>) -> Result<Self> {
        Ok(if input.bit()? {
            Self::Copy
        } else {
            Self::Residual
        })
    }

    fn next(self, _: &mut Bits<'_>) -> Result<Self> {
        Ok(self.toggle())
    }
}

impl<'a, T: RunSymbol> RunStream<'a, T> {
    fn new(mut input: Bits<'a>, tree: &Tree) -> Result<Self> {
        let (value, remaining) = if input.data.is_empty() {
            (None, 0)
        } else {
            (
                Some(T::initial(&mut input)?),
                tree.escaped(&mut input, -1, 255)? as u32,
            )
        };
        Ok(Self {
            input,
            value,
            remaining,
        })
    }

    fn next(&mut self, tree: &Tree) -> Result<T> {
        let mut value = self.value.ok_or(Error::Truncated)?;
        if self.remaining == 0 {
            value = value.next(&mut self.input)?;
            self.value = Some(value);
            self.remaining = tree.escaped(&mut self.input, -1, 255)? as u32;
        }
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(Error::Invalid("empty macroblock run"))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{vec, vec::Vec};

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
        tree.read(&mut input, SymbolKind::Signed, 2).unwrap();
        for value in expected {
            assert_eq!(tree.symbol(&mut input).unwrap(), i32::from(value as i8) * 4);
        }
    }

    #[test]
    fn trees_are_bounded_and_truncated_codes_fail() {
        let mut tree = Tree::new();
        assert!(matches!(
            tree.read(&mut Bits::new(&[255; 40]), SymbolKind::Unsigned, 0),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            tree.read(&mut Bits::new(&[0]), SymbolKind::Unsigned, 0),
            Err(Error::Truncated)
        ));
        let mut bits = vec![true];
        leaf(&mut bits, 0);
        leaf(&mut bits, 1);
        let bytes = pack(&bits);
        tree.read(&mut Bits::new(&bytes), SymbolKind::Unsigned, 0)
            .unwrap();
        assert!(matches!(
            tree.symbol(&mut Bits::default()),
            Err(Error::Truncated)
        ));
    }

    #[test]
    fn constant_escape_is_rejected_without_looping() {
        let mut tree = Tree::new();
        tree.values[0] = 127;
        assert!(matches!(
            tree.escaped(&mut Bits::default(), -128, 127),
            Err(Error::Invalid("nonterminating Huffman escape"))
        ));
    }

    #[test]
    fn motion_vectors_stay_within_the_coded_range() {
        let mut tree = Tree::new();
        for (symbol, mut value, expected) in [(31, 7936, -512), (-32, -8192, 0)] {
            tree.values[0] = symbol;
            advance_motion(&tree, &mut Bits::new(&[0]), 8, &mut value).unwrap();
            assert_eq!(value, expected);
        }
        for symbol in [-128, 127] {
            tree.values[0] = symbol;
            assert!(matches!(
                advance_motion(&tree, &mut Bits::new(&[0]), 8, &mut 0),
                Err(Error::Invalid(_))
            ));
        }
    }
}
