//! Reusable decoder state and picture reconstruction orchestration.
mod motion;
mod plane;
mod transform;

use crate::{
    entropy::Streams,
    error::{Error, Result},
    syntax::{
        BlockCoding, IntraBlock, MacroblockMode, MotionMode, MotionVector, Orientation, PlaneGroup,
        PlaneId, Planes, PredictedBlock, Reference,
    },
    BlockState, BufferKind, Frame, FrameType, Limits, VideoInfo,
};
use motion::MotionSample;
use plane::{Layout, Plane};
use transform::{intra_block, predicted_block, weighted, Nest};

/// Reusable storage accepted by [`VideoDecoder::with_buffers`].
///
/// Each frame needs `VideoInfo::buffer_requirements().frame_bytes` bytes;
/// `blocks` needs `block_states` elements. Slices, arrays, and (with `alloc`)
/// vectors all work. The decoder owns these handles for its lifetime, so its
/// reference frames cannot accidentally be replaced between decode calls.
/// Larger buffers are allowed; only the required prefixes are used.
pub struct DecoderBuffers<F, B> {
    /// Three independent, tightly packed Y/U/V frame buffers.
    pub frames: [F; 3],
    /// Reusable block-descriptor workspace.
    pub blocks: B,
}

struct Frames<F> {
    buffers: [F; 3],
    current: usize,
    past: usize,
    future: usize,
}
impl<F> Frames<F> {
    fn begin(&mut self, kind: FrameType) -> [&mut F; 3] {
        if kind != FrameType::B {
            core::mem::swap(&mut self.past, &mut self.future);
        }
        self.buffers
            .get_disjoint_mut([self.current, self.past, self.future])
            .expect("frame indices are distinct")
    }
    fn finish(&mut self, kind: FrameType) -> &F {
        match kind {
            FrameType::I | FrameType::P => {
                core::mem::swap(&mut self.current, &mut self.future);
                &self.buffers[self.future]
            }
            FrameType::B => &self.buffers[self.current],
        }
    }
}

#[derive(Clone, Copy)]
enum DecodeState {
    NeedIntra,
    OneReference,
    TwoReferences,
    Failed,
}
impl DecodeState {
    fn after(self, kind: FrameType) -> Result<Self> {
        match (self, kind) {
            (Self::Failed, _) => Err(Error::Failed),
            (Self::NeedIntra, FrameType::P | FrameType::B) => {
                Err(Error::Invalid("prediction before an I frame"))
            }
            (Self::OneReference, FrameType::B) => {
                Err(Error::Invalid("B frame needs past and future references"))
            }
            (Self::NeedIntra, FrameType::I) => Ok(Self::OneReference),
            _ => Ok(Self::TwoReferences),
        }
    }
}

/// Stateful, allocation-free decoder for demuxed video packets.
///
/// [`Self::with_buffers`] accepts caller-owned storage and works without `alloc`
/// or `std`. With the `alloc` feature, `VideoDecoder::new` allocates that storage once.
/// Decoding never allocates. Returned frames borrow reusable storage; copy them
/// before the next call if needed. Packets omit the initial four-byte display ID.
pub struct VideoDecoder<F, B> {
    info: VideoInfo,
    layouts: Planes<Layout>,
    frames: Frames<F>,
    blocks: B,
    state: DecodeState,
    nest: [u8; 70 * 38],
    max_packet: usize,
}

#[cfg(feature = "alloc")]
impl VideoDecoder<alloc::vec::Vec<u8>, alloc::vec::Vec<BlockState>> {
    /// Allocate reusable buffers with the default resource limits.
    pub fn new(info: VideoInfo) -> Result<Self> {
        Self::with_limits(info, Limits::default())
    }
    /// Allocate reusable buffers after checking explicit resource limits.
    pub fn with_limits(info: VideoInfo, limits: Limits) -> Result<Self> {
        info.validate_limits(limits)?;
        let requirements = info.buffer_requirements();
        Self::with_buffers(
            info,
            DecoderBuffers {
                frames: core::array::from_fn(|_| alloc::vec![0; requirements.frame_bytes]),
                blocks: alloc::vec![BlockState::EMPTY; requirements.block_states],
            },
            limits,
        )
    }
}

impl<F: AsRef<[u8]> + AsMut<[u8]>, B: AsMut<[BlockState]>> VideoDecoder<F, B> {
    /// Validate and initialize caller-owned storage without allocating.
    ///
    /// Size checks happen before initialization. The used frame prefixes are
    /// cleared, and the descriptor workspace is reset. Retaining the buffers
    /// here keeps their reference-picture history valid across calls.
    pub fn with_buffers(
        info: VideoInfo,
        mut buffers: DecoderBuffers<F, B>,
        limits: Limits,
    ) -> Result<Self> {
        info.validate_limits(limits)?;
        let requirements = info.buffer_requirements();
        for frame in &mut buffers.frames {
            let provided = frame.as_mut().len().min(frame.as_ref().len());
            if provided < requirements.frame_bytes {
                return Err(Error::BufferTooSmall {
                    buffer: BufferKind::Frame,
                    required: requirements.frame_bytes,
                    provided,
                });
            }
        }
        let blocks = buffers.blocks.as_mut();
        if blocks.len() < requirements.block_states {
            return Err(Error::BufferTooSmall {
                buffer: BufferKind::BlockStates,
                required: requirements.block_states,
                provided: blocks.len(),
            });
        }
        blocks[..requirements.block_states].fill(BlockState::EMPTY);
        for frame in &mut buffers.frames {
            frame.as_mut()[..requirements.frame_bytes].fill(0);
        }
        Ok(Self {
            info,
            layouts: plane::layouts(info),
            frames: Frames {
                buffers: buffers.frames,
                current: 0,
                past: 1,
                future: 2,
            },
            blocks: buffers.blocks,
            state: DecodeState::NeedIntra,
            nest: [0; 70 * 38],
            max_packet: limits.max_frame_bytes,
        })
    }
    /// The validated layout used by this decoder.
    pub fn info(&self) -> VideoInfo {
        self.info
    }
    /// Recover the owned or borrowed buffers in their original order.
    pub fn into_buffers(self) -> DecoderBuffers<F, B> {
        DecoderBuffers {
            frames: self.frames.buffers,
            blocks: self.blocks,
        }
    }
    /// Decode one packet without allocating or copying a complete frame.
    ///
    /// Any error poisons the decoder because references may be partly modified.
    /// Subsequent calls return [`Error::Failed`]; construct a new decoder to retry.
    pub fn decode(
        &mut self,
        kind: FrameType,
        display_index: u32,
        packet: &[u8],
    ) -> Result<Frame<'_>> {
        let previous = core::mem::replace(&mut self.state, DecodeState::Failed);
        let next = previous.after(kind)?;
        if packet.len() > self.max_packet {
            return Err(Error::Limit("compressed frame size"));
        }
        let mut streams = Streams::new(packet, kind)?;
        let [current, past, future] = self.frames.begin(kind);
        let mut picture = Picture {
            info: self.info,
            planes: plane::borrow_planes(&self.layouts, self.blocks.as_mut()),
            destination: current.as_mut(),
            past: past.as_ref(),
            future: future.as_ref(),
            nest: &mut self.nest,
        };
        match kind {
            FrameType::I => picture.intra(&mut streams)?,
            FrameType::P | FrameType::B => picture.inter(&mut streams, kind)?,
        }
        self.state = next;
        let data = self.frames.finish(kind).as_ref();
        Ok(Frame::new(
            self.info,
            kind,
            display_index,
            &data[..self.info.buffer_requirements().frame_bytes],
        ))
    }
}

/// A temporary, disjoint view of storage for a single decode operation.
struct Picture<'a> {
    info: VideoInfo,
    planes: Planes<Plane<'a>>,
    destination: &'a mut [u8],
    past: &'a [u8],
    future: &'a [u8],
    nest: &'a mut [u8; 70 * 38],
}
impl Picture<'_> {
    fn orientation(&self) -> Orientation {
        if self.info.width() >= self.info.height() {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }
    fn set_block_types(
        &mut self,
        group: PlaneGroup,
        x: usize,
        y: usize,
        mode: MacroblockMode,
        codes: u8,
    ) -> Result<()> {
        match group {
            PlaneGroup::Luma => {
                let plane = &mut self.planes.y;
                plane.blocks[plane.layout.index(x, y)].coding = BlockCoding::parse(mode, codes)?;
            }
            PlaneGroup::Chroma => {
                for (component, code) in [(PlaneId::U, codes & 15), (PlaneId::V, codes >> 4)] {
                    let plane = &mut self.planes[component];
                    plane.blocks[plane.layout.index(x, y)].coding = BlockCoding::parse(mode, code)?;
                }
            }
        }
        Ok(())
    }
    fn group_layout(&self, group: PlaneGroup) -> Layout {
        match group {
            PlaneGroup::Luma => self.planes.y.layout,
            PlaneGroup::Chroma => self.planes.u.layout,
        }
    }
    fn intra(&mut self, streams: &mut Streams<'_>) -> Result<()> {
        for group in PlaneGroup::ALL {
            let mut remaining = 0;
            let layout = self.group_layout(group);
            for y in 0..layout.block_height {
                for x in 0..layout.block_width {
                    self.set_block_types(
                        group,
                        x,
                        y,
                        MacroblockMode::Intra,
                        streams.block_types(group, &mut remaining)?,
                    )?;
                }
            }
        }
        for (component, plane) in self.planes.iter_mut() {
            let layout = plane.layout;
            let mut remaining = 0;
            for y in 0..layout.block_height {
                let mut prediction = plane.blocks[layout.index(0, y) - layout.descriptor_stride].dc;
                for x in 0..layout.block_width {
                    let index = layout.index(x, y);
                    let dc =
                        prediction.wrapping_add(streams.delta(component, &mut remaining)? as u8);
                    plane.blocks[index].dc = dc;
                    prediction = (u16::from(dc)
                        + u16::from(plane.blocks[index - layout.descriptor_stride + 1].dc))
                    .div_ceil(2) as u8;
                }
            }
        }
        let (nest_x, nest_y) = streams.nest_origin()?;
        self.make_nest(nest_x, nest_y)?;
        let nest = Nest::dc(self.nest, self.orientation());
        for (component, plane) in self.planes.iter() {
            let layout = plane.layout;
            for y in 0..layout.block_height {
                for x in 0..layout.block_width {
                    let block = plane.blocks[layout.index(x, y)];
                    let pixels = match block.coding {
                        BlockCoding::Intra(IntraBlock::Weighted) => {
                            let neighbors = [
                                layout.index(x, y.saturating_sub(1)),
                                layout.index(x, (y + 1).min(layout.block_height - 1)),
                                layout.index(x.saturating_sub(1), y),
                                layout.index((x + 1).min(layout.block_width - 1), y),
                            ];
                            weighted(
                                block.dc,
                                neighbors.map(|index| neighbor(plane.blocks[index], block.dc)),
                            )
                        }
                        BlockCoding::Intra(coding) => {
                            intra_block(streams, component, coding, block.dc, nest)?
                        }
                        _ => return Err(Error::Invalid("non-intra block in intra picture")),
                    };
                    layout.put(self.destination, x, y, &pixels);
                }
            }
        }
        Ok(())
    }
    fn make_nest(&mut self, x: usize, y: usize) -> Result<()> {
        let plane = &self.planes.y;
        let layout = plane.layout;
        let (nest_width, nest_height) = self.orientation().nest_dimensions();
        let width = layout.block_width.min(nest_width);
        let height = layout.block_height.min(nest_height);
        if x + width > layout.block_width || y + height > layout.block_height {
            return Err(Error::Invalid("DC nest outside luma plane"));
        }
        self.nest.fill(0);
        for row in 0..nest_height.min(height * 2) {
            let source_y = if row < height {
                row
            } else {
                height * 2 - 1 - row
            };
            for column in 0..nest_width.min(width * 2) {
                let source_x = if column < width {
                    column
                } else {
                    width * 2 - 1 - column
                };
                self.nest[row * nest_width + column] =
                    plane.blocks[layout.index(x + source_x, y + source_y)].dc >> 4;
            }
        }
        Ok(())
    }
    fn descriptors(&mut self, streams: &mut Streams<'_>) -> Result<()> {
        let mut dc = Planes::from_fn(|_| 127u8);
        let (mut luma_run, mut chroma_run) = (0, 0);
        for macro_y in 0..usize::from(self.info.height()) / 8 {
            for macro_x in 0..usize::from(self.info.width()) / 8 {
                let mode = streams.macroblock_mode()?;
                match mode {
                    MacroblockMode::Intra => {
                        for (component, plane) in self.planes.iter_mut() {
                            for sub in 0..plane.layout.blocks_per_macroblock() {
                                dc[component] =
                                    dc[component].wrapping_add(streams.dc(component)? as u8);
                                let (x, y) = plane.layout.macroblock(macro_x, macro_y, sub);
                                plane.blocks[plane.layout.index(x, y)].dc = dc[component];
                            }
                        }
                    }
                    MacroblockMode::Predicted { .. } => dc = Planes::from_fn(|_| 127),
                }
                for (group, remaining) in [
                    (PlaneGroup::Luma, &mut luma_run),
                    (PlaneGroup::Chroma, &mut chroma_run),
                ] {
                    let layout = self.group_layout(group);
                    for sub in 0..layout.blocks_per_macroblock() {
                        let code = match mode {
                            MacroblockMode::Predicted {
                                motion: MotionMode::Copy,
                                ..
                            } => 0,
                            _ => streams.block_types(group, remaining)?,
                        };
                        let (x, y) = layout.macroblock(macro_x, macro_y, sub);
                        self.set_block_types(group, x, y, mode, code)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn inter(&mut self, streams: &mut Streams<'_>, kind: FrameType) -> Result<()> {
        self.descriptors(streams)?;
        let orientation = self.orientation();
        let mut reference = None;
        let mut motion = MotionVector::default();
        for macro_y in 0..usize::from(self.info.height()) / 8 {
            for macro_x in 0..usize::from(self.info.width()) / 8 {
                let first = self.planes.y.blocks
                    [self.planes.y.layout.index(macro_x * 2, macro_y * 2)]
                .coding;
                if let BlockCoding::Predicted {
                    reference: selected,
                    ..
                } = first
                {
                    if reference != Some(selected) {
                        reference = Some(selected);
                        motion = MotionVector::default();
                    }
                    streams.motion(selected, &mut motion)?;
                }
                let position = MotionVector {
                    x: macro_x as i32 * 16 + motion.x,
                    y: macro_y as i32 * 16 + motion.y,
                };
                for (component, plane) in self.planes.iter() {
                    let layout = plane.layout;
                    for sub in 0..layout.blocks_per_macroblock() {
                        let (x, y) = layout.macroblock(macro_x, macro_y, sub);
                        let index = layout.index(x, y);
                        let block = plane.blocks[index];
                        let pixels = match block.coding {
                            BlockCoding::Intra(IntraBlock::Weighted) => {
                                let stride = layout.descriptor_stride;
                                weighted(
                                    block.dc,
                                    [index - stride, index + stride, index - 1, index + 1]
                                        .map(|index| neighbor(plane.blocks[index], block.dc)),
                                )
                            }
                            BlockCoding::Intra(coding) => intra_block(
                                streams,
                                component,
                                coding,
                                block.dc,
                                Nest::dc(self.nest, orientation),
                            )?,
                            BlockCoding::Predicted {
                                block: PredictedBlock::Literal,
                                ..
                            } => streams.literal(component)?,
                            BlockCoding::Predicted {
                                reference,
                                block: coding,
                            } => {
                                let sample =
                                    MotionSample::new(layout, position, sub, self.info.version())?;
                                if reference == Reference::Second
                                    && kind == FrameType::P
                                    && coding == PredictedBlock::Copy
                                {
                                    sample
                                        .copy_within(self.destination, layout.sample_offset(x, y));
                                    continue;
                                }
                                let source: &[u8] = match (reference, kind) {
                                    (Reference::Past, _) => self.past,
                                    (Reference::Second, FrameType::P) => self.destination,
                                    (Reference::Second, _) => self.future,
                                };
                                let predicted = sample.read(source);
                                match coding {
                                    PredictedBlock::Copy => predicted,
                                    PredictedBlock::Transform(bases) => {
                                        let luma = self.planes.y.layout;
                                        let nest = Nest::reference(
                                            &source[..luma.width * luma.height],
                                            luma.width,
                                            position,
                                            orientation,
                                        );
                                        predicted_block(streams, component, bases, predicted, nest)?
                                    }
                                    PredictedBlock::Literal => {
                                        return Err(Error::Invalid(
                                            "literal block in motion reconstruction",
                                        ))
                                    }
                                }
                            }
                            BlockCoding::Border => {
                                return Err(Error::Invalid("border block in picture"))
                            }
                        };
                        layout.put(self.destination, x, y, &pixels);
                    }
                }
            }
        }
        Ok(())
    }
}

fn neighbor(block: BlockState, fallback: u8) -> u8 {
    if block.coding.dc_neighbor() {
        block.dc
    } else {
        fallback
    }
}
