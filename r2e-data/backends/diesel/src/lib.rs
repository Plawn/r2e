pub mod error;
pub mod repository;

pub use error::DieselErrorExt;
pub use repository::DieselRepository;

pub mod prelude {
    pub use crate::{DieselErrorExt, DieselRepository};
    pub use r2e_data::prelude::*;
}
