//! Values every crate of the core shares.

use std::fmt;

/// The core's scalar. The core computes in 64-bit floats.
pub type Real = f64;

/// The scalar of the engine-order vectors. The reference implementation stores vectors in single
/// precision, so the `reference` configuration does too.
#[cfg(not(feature = "reference"))]
pub type Ef = f64;
#[cfg(feature = "reference")]
pub type Ef = f32;

/// A plane vector. Its operations follow the engine's order of operations.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: Ef,
    pub y: Ef,
}

/// A space vector, with y up. Its operations follow the engine's order of operations.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: Ef,
    pub y: Ef,
    pub z: Ef,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec4 {
    pub x: Ef,
    pub y: Ef,
    pub z: Ef,
    pub w: Ef,
}

/// A 3x3 matrix stored by rows, as the engine stores it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Basis {
    pub rows: [Vec3; 3],
}

/// A seed. Derivation wraps in 64-bit signed arithmetic, as the reference's integers do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seed(pub i64);

macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The name the reference's data uses.
            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn from_name(text: &str) -> Option<$name> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

named_enum! {
    /// One of the 14 clubs of the standard bag. The discriminant is the club's place in the bag,
    /// from the driver to the putter.
    ClubId {
        Driver = "driver",
        Wood3 = "3wood",
        Hybrid = "hybrid",
        Iron4 = "4iron",
        Iron5 = "5iron",
        Iron6 = "6iron",
        Iron7 = "7iron",
        Iron8 = "8iron",
        Iron9 = "9iron",
        PitchingWedge = "pw",
        GapWedge = "gw",
        SandWedge = "sw",
        LobWedge = "lw",
        Putter = "putter",
    }
}

named_enum! {
    /// The kind of ground the ball meets. The discriminant is the reference's enum value, and the
    /// name is the one a course bundle's materials use.
    Surface {
        Tee = "TEE",
        Fairway = "FAIRWAY",
        FirstCut = "FIRST_CUT",
        Rough = "ROUGH",
        DeepRough = "DEEP_ROUGH",
        Fringe = "FRINGE",
        Green = "GREEN",
        Bunker = "BUNKER",
        Hardpan = "HARDPAN",
        CartPath = "CART_PATH",
        Water = "WATER",
    }
}

/// A course material: an index into the course's material table, as its material grids store it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Material(pub u8);

/// Why the core could not read or reproduce something. A `path` names one value, such as
/// `holes[2].cups[0].position` or `cases[12].output.events[3].time_s`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    Io {
        file: String,
        message: String,
    },
    /// Bytes that do not decode as their format says.
    Malformed {
        path: String,
        message: String,
    },
    MissingKey {
        path: String,
    },
    WrongType {
        path: String,
        expected: &'static str,
    },
    /// A format, bundle or record version this build does not read.
    UnknownVersion {
        what: &'static str,
        version: i64,
    },
    /// A course whose bytes do not hash to the revision its manifest names.
    RevisionMismatch {
        expected: String,
        actual: String,
    },
    /// A stored result that differs from the one the core computes, at its first difference.
    Diverged {
        entry: usize,
        path: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { file, message } => write!(f, "{file}: {message}"),
            Error::Malformed { path, message } => write!(f, "{path}: {message}"),
            Error::MissingKey { path } => write!(f, "{path}: missing"),
            Error::WrongType { path, expected } => write!(f, "{path}: expected {expected}"),
            Error::UnknownVersion { what, version } => {
                write!(f, "unknown {what} version {version}")
            }
            Error::RevisionMismatch { expected, actual } => {
                write!(
                    f,
                    "course revision {actual} differs from the manifest's {expected}"
                )
            }
            Error::Diverged { entry, path } => write!(f, "entry {entry} diverges at {path}"),
        }
    }
}

impl std::error::Error for Error {}

/// The kind of decision the core waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptKind {
    Play,
    MaybeLost,
    Choose,
    Drop,
    Place,
    Done,
}

/// Why the core refused an action. A refused action changes nothing.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Refusal {
    /// The action does not answer the prompt the core waits on.
    NotNow {
        prompt: PromptKind,
    },
    /// A choice or a spot that the prompt did not offer.
    NotOffered,
    OutOfRange {
        setting: &'static str,
        value: Real,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NotNow { prompt } => write!(f, "not now: the prompt is {prompt:?}"),
            Refusal::NotOffered => f.write_str("not one of the options offered"),
            Refusal::OutOfRange { setting, value } => {
                write!(f, "{setting} {value} is out of range")
            }
        }
    }
}

impl std::error::Error for Refusal {}
