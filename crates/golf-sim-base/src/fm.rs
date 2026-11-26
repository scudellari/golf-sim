//! Portable floating-point functions backed by the workspace's soft-float libm.

pub use libm::{
    acos, acosh, asin, asinh, atan, atan2, atanh, cbrt, ceil, cos, cosh, exp, exp2, expm1, floor,
    fmod, hypot, log, log1p, log2, log10, pow, round, sin, sincos, sinh, sqrt, tan, tanh,
};
