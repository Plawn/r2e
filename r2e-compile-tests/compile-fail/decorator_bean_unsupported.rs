//! `#[derive(DecoratorBean)]` rejects shapes it cannot support with a clear
//! message: enums, tuple structs, and generic types.

use r2e::prelude::*;

#[derive(DecoratorBean)]
pub enum NotAStruct {
    A,
}

#[derive(DecoratorBean)]
pub struct TupleShape(pub u64);

#[derive(DecoratorBean)]
pub struct GenericGuard<T> {
    #[inject]
    dep: T,
}

fn main() {}
