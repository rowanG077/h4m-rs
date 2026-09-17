//! Typed picture syntax. Raw codes are interpreted once, at the bitstream boundary.

use crate::error::{Error, Result};
use core::ops::{Index, IndexMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneId {
    Y,
    U,
    V,
}

/// Storage indexed by a component name rather than a numeric stream convention.
pub(crate) struct Planes<T> {
    pub y: T,
    pub u: T,
    pub v: T,
}

impl<T> Planes<T> {
    pub fn from_fn(mut f: impl FnMut(PlaneId) -> T) -> Self {
        Self {
            y: f(PlaneId::Y),
            u: f(PlaneId::U),
            v: f(PlaneId::V),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (PlaneId, &T)> {
        [
            (PlaneId::Y, &self.y),
            (PlaneId::U, &self.u),
            (PlaneId::V, &self.v),
        ]
        .into_iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (PlaneId, &mut T)> {
        [
            (PlaneId::Y, &mut self.y),
            (PlaneId::U, &mut self.u),
            (PlaneId::V, &mut self.v),
        ]
        .into_iter()
    }
}

impl<T> Index<PlaneId> for Planes<T> {
    type Output = T;
    fn index(&self, plane: PlaneId) -> &T {
        match plane {
            PlaneId::Y => &self.y,
            PlaneId::U => &self.u,
            PlaneId::V => &self.v,
        }
    }
}

impl<T> IndexMut<PlaneId> for Planes<T> {
    fn index_mut(&mut self, plane: PlaneId) -> &mut T {
        match plane {
            PlaneId::Y => &mut self.y,
            PlaneId::U => &mut self.u,
            PlaneId::V => &mut self.v,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PlaneGroup {
    Luma,
    Chroma,
}

impl PlaneGroup {
    pub const ALL: [Self; 2] = [Self::Luma, Self::Chroma];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reference {
    Past,
    Second,
}

/// Wire codes in the macroblock reference stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum PredictionTarget {
    Intra = 0,
    Past = 1,
    Second = 2,
}

impl TryFrom<u8> for PredictionTarget {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Intra),
            1 => Ok(Self::Past),
            2 => Ok(Self::Second),
            _ => Err(Error::Invalid("macroblock reference")),
        }
    }
}

impl PredictionTarget {
    pub fn reference(self) -> Option<Reference> {
        match self {
            Self::Intra => None,
            Self::Past => Some(Reference::Past),
            Self::Second => Some(Reference::Second),
        }
    }

    pub fn transition(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Intra, false) | (Self::Second, true) => Self::Past,
            (Self::Past, false) | (Self::Intra, true) => Self::Second,
            (Self::Second, false) | (Self::Past, true) => Self::Intra,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MotionMode {
    Residual,
    Copy,
}

impl MotionMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Residual => Self::Copy,
            Self::Copy => Self::Residual,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MacroblockMode {
    Intra,
    Predicted {
        reference: Reference,
        motion: MotionMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntraBlock {
    Weighted,
    Transform(u8),
    Literal,
    Solid,
}

impl TryFrom<u8> for IntraBlock {
    type Error = Error;

    fn try_from(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Weighted),
            1..=5 => Ok(Self::Transform(code)),
            6 => Ok(Self::Literal),
            8 => Ok(Self::Solid),
            _ => Err(Error::Invalid("intra block type")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PredictedBlock {
    Copy,
    Transform(u8),
    Literal,
}

impl TryFrom<u8> for PredictedBlock {
    type Error = Error;

    fn try_from(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Copy),
            1..=5 => Ok(Self::Transform(code - 1)),
            6 => Ok(Self::Literal),
            _ => Err(Error::Invalid("predictive block type")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockCoding {
    Border,
    Intra(IntraBlock),
    Predicted {
        reference: Reference,
        block: PredictedBlock,
    },
}

impl BlockCoding {
    pub fn parse(mode: MacroblockMode, code: u8) -> Result<Self> {
        match mode {
            MacroblockMode::Intra => Ok(Self::Intra(code.try_into()?)),
            MacroblockMode::Predicted { reference, .. } => Ok(Self::Predicted {
                reference,
                block: code.try_into()?,
            }),
        }
    }

    pub fn dc_neighbor(self) -> bool {
        matches!(self, Self::Intra(IntraBlock::Weighted | IntraBlock::Solid))
    }
}

/// One element of the caller-provided block-descriptor workspace.
///
/// Contents are private decoder state. Allocate `VideoInfo::buffer_requirements().block_states`
/// elements initialized with [`Self::EMPTY`]; the decoder initializes them again
/// when the buffers are attached. No allocation is performed by this type.
#[derive(Debug, Clone, Copy)]
pub struct BlockState {
    pub(crate) dc: u8,
    pub(crate) coding: BlockCoding,
}

impl BlockState {
    /// Initial value for arrays and other caller-owned storage.
    pub const EMPTY: Self = Self {
        dc: 127,
        coding: BlockCoding::Border,
    };
}

impl Default for BlockState {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    pub fn nest_dimensions(self) -> (usize, usize) {
        match self {
            Self::Landscape => (70, 38),
            Self::Portrait => (38, 70),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MotionVector {
    pub x: i32,
    pub y: i32,
}
