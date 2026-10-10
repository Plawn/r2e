use r2e::prelude::*;

#[derive(Debug, ApiError)]
pub enum FixedStatus {
    #[error(rejection, status = BAD_REQUEST)]
    Rejected(Rejection),
}

fn main() {}
