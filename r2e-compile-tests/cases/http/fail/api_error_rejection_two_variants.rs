use r2e::prelude::*;

#[derive(Debug, ApiError)]
pub enum TwoRejections {
    #[error(rejection)]
    First(Rejection),
    #[error(rejection)]
    Second(Rejection),
}

fn main() {}
