//! `after(..)` exists to name a dependency the producer body does NOT read.
//! A type that is already a parameter is already an edge, so listing it in
//! `after(..)` is either a misunderstanding or a leftover — and it would push a
//! second, identical entry into `dependencies()`.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Settings;

#[derive(Clone)]
pub struct Db;

#[producer(after(Settings))]
fn create_db(settings: Settings) -> Db {
    let _ = settings;
    Db
}

fn main() {}
