use r2e::prelude::*;

#[derive(Debug, ApiError)]
pub enum BadShape {
    #[error(rejection)]
    Rejected(Rejection, String),
}

#[derive(Debug, ApiError)]
pub enum NoField {
    #[error(rejection)]
    Rejected,
}

fn main() {}
