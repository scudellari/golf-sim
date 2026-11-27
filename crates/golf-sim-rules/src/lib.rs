pub mod relief;
pub mod rulings;
pub mod transitions;

pub use relief::{AbnormalCondition, AbnormalKind, ReliefReason};

/// The rules model version stored in hole records.
pub const VERSION: &str = "0.3.1";
