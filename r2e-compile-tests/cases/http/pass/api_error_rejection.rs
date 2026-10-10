use r2e::prelude::*;

#[derive(Debug, ApiError)]
pub enum ApiEnvelope {
    #[error(status = CONFLICT)]
    Duplicate,
    #[error(rejection)]
    Rejected(Rejection),
}

fn assert_envelope<E: From<Rejection> + IntoHttpResponse + ErrorSchema>() {}

fn main() {
    assert_envelope::<ApiEnvelope>();
    assert_envelope::<HttpError>();
    let _: ApiEnvelope = Rejection::not_found("x").into();
}
