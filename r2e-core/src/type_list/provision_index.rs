//! Compile-time membership checks for the final provision list.

use super::{Contains, TCons, TNil};
use std::marker::PhantomData;

/// Groups a provision list into eight-slot blocks for dependency checking at
/// `build_state()`. The builder still materializes the original list as its state.
pub trait ProvisionIndex {
    /// Type against which the builder checks each registered dependency.
    type Index;
}

// These types occur only in inferred proofs. A block can match any of its
// eight slots directly, or continue the search in the next block.
pub struct Block<Slots, Tail>(PhantomData<fn() -> (Slots, Tail)>);
pub struct Slot<const N: usize>;
pub struct Next<I>(PhantomData<fn() -> I>);

#[diagnostic::do_not_recommend]
impl<Slots, Tail, Bean, I> Contains<Bean, Next<I>> for Block<Slots, Tail> where
    Tail: Contains<Bean, I>
{
}

macro_rules! block_slots {
    ($(($bean:ident, $index:literal)),+) => {
        $(#[diagnostic::do_not_recommend]
        impl<A, B, C, D, E, F, G, H, Tail> Contains<$bean, Slot<$index>>
            for Block<(A, B, C, D, E, F, G, H), Tail> {})+
    };
}

block_slots!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);

macro_rules! list {
    ($head:ident => $tail:ty) => { TCons<$head, $tail> };
    ($head:ident, $($rest:ident),+ => $tail:ty) => {
        TCons<$head, list!($($rest),+ => $tail)>
    };
}

// The final zero to seven slots already implement Contains as a short TCons
// list. Matching an exact length here keeps these impls disjoint from the
// recursive case below.
impl ProvisionIndex for TNil {
    type Index = Self;
}

macro_rules! short_list {
    ($($slot:ident),+) => {
        impl<$($slot),+> ProvisionIndex for list!($($slot),+ => TNil) {
            type Index = Self;
        }
    };
}

short_list!(A);
short_list!(A, B);
short_list!(A, B, C);
short_list!(A, B, C, D);
short_list!(A, B, C, D, E);
short_list!(A, B, C, D, E, F);
short_list!(A, B, C, D, E, F, G);

impl<A, B, C, D, E, F, G, H, Tail: ProvisionIndex> ProvisionIndex for list!(A, B, C, D, E, F, G, H => Tail) {
    type Index = Block<(A, B, C, D, E, F, G, H), Tail::Index>;
}
