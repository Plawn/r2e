//! The canonical bean graph reused across DI test modules:
//! `Dep` feeds `ServiceA`, which feeds `ServiceB`.

#![allow(dead_code)]

use std::any::{type_name, TypeId};

use r2e_core::beans::{Bean, BeanContext};
use r2e_core::type_list::TNil;

#[derive(Clone)]
pub struct Dep {
    pub value: i32,
}

#[derive(Clone)]
pub struct ServiceA {
    pub dep: Dep,
}

impl Bean for ServiceA {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            dep: ctx.get::<Dep>(),
        }
    }
}

#[derive(Clone)]
pub struct ServiceB {
    pub a: ServiceA,
    pub dep: Dep,
}

impl Bean for ServiceB {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![
            (TypeId::of::<ServiceA>(), type_name::<ServiceA>()),
            (TypeId::of::<Dep>(), type_name::<Dep>()),
        ]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            a: ctx.get::<ServiceA>(),
            dep: ctx.get::<Dep>(),
        }
    }
}
