pub mod bag;
pub mod batch;
pub mod corpus;
pub mod course;
pub mod day;
pub mod digests;
pub mod explain;
pub mod fit;
pub mod play;
pub mod play_course;
pub mod probe;
pub mod reach;
pub mod replay;
pub mod scenario;
pub mod schema;
pub mod strategy;
pub mod stroke;
pub mod sweep;
pub mod tables;
pub mod validate;

pub type Outcome = Result<(), Box<dyn std::error::Error>>;

pub fn not_built(command: &str) -> Outcome {
    Err(format!("`golf {command}` is not implemented yet").into())
}
