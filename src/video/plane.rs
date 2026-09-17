//! Plane geometry and borrowed block-descriptor storage.
use crate::{
    syntax::{PlaneId, Planes},
    BlockState, VideoInfo,
};

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub width: usize,
    pub height: usize,
    pub offset: usize,
    pub horizontal_shift: u8,
    pub vertical_shift: u8,
    pub block_width: usize,
    pub block_height: usize,
    pub descriptor_stride: usize,
}
impl Layout {
    fn new(info: VideoInfo, component: PlaneId, offset: usize) -> Self {
        let horizontal_shift =
            u8::from(component != PlaneId::Y && info.sampling().horizontal_factor() == 2);
        let vertical_shift =
            u8::from(component != PlaneId::Y && info.sampling().vertical_factor() == 2);
        let width = usize::from(info.width()) >> horizontal_shift;
        let height = usize::from(info.height()) >> vertical_shift;
        Self {
            width,
            height,
            offset,
            horizontal_shift,
            vertical_shift,
            block_width: width / 4,
            block_height: height / 4,
            descriptor_stride: width / 4 + 2,
        }
    }
    pub fn descriptor_count(self) -> usize {
        self.descriptor_stride * (self.block_height + 2)
    }
    pub fn index(self, x: usize, y: usize) -> usize {
        (y + 1) * self.descriptor_stride + x + 1
    }
    pub fn macroblock(self, x: usize, y: usize, sub: usize) -> (usize, usize) {
        // The format traverses a macroblock clockwise from the top left.
        const SUBBLOCKS: [(usize, usize); 4] = [(0, 0), (0, 1), (1, 1), (1, 0)];
        (
            ((x * 2) >> self.horizontal_shift) + SUBBLOCKS[sub].0,
            ((y * 2) >> self.vertical_shift) + SUBBLOCKS[sub].1,
        )
    }
    pub fn blocks_per_macroblock(self) -> usize {
        4 >> (self.horizontal_shift + self.vertical_shift)
    }
    pub fn sample_offset(self, x: usize, y: usize) -> usize {
        self.offset + y * 4 * self.width + x * 4
    }
    pub fn put(self, destination: &mut [u8], x: usize, y: usize, pixels: &[u8; 16]) {
        let start = self.sample_offset(x, y);
        for (row, pixels) in pixels.as_chunks::<4>().0.iter().enumerate() {
            destination[start + row * self.width..start + row * self.width + 4]
                .copy_from_slice(pixels);
        }
    }
}

pub(super) struct Plane<'a> {
    pub layout: Layout,
    pub blocks: &'a mut [BlockState],
}

pub(super) fn layouts(info: VideoInfo) -> Planes<Layout> {
    let mut offset = 0;
    Planes::from_fn(|component| {
        let layout = Layout::new(info, component, offset);
        offset += layout.width * layout.height;
        layout
    })
}
pub(super) fn borrow_planes<'a>(
    layouts: &Planes<Layout>,
    blocks: &'a mut [BlockState],
) -> Planes<Plane<'a>> {
    let (y, rest) = blocks.split_at_mut(layouts.y.descriptor_count());
    let (u, rest) = rest.split_at_mut(layouts.u.descriptor_count());
    let (v, _) = rest.split_at_mut(layouts.v.descriptor_count());
    Planes {
        y: Plane {
            layout: layouts.y,
            blocks: y,
        },
        u: Plane {
            layout: layouts.u,
            blocks: u,
        },
        v: Plane {
            layout: layouts.v,
            blocks: v,
        },
    }
}
