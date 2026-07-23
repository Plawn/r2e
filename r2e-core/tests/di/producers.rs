//! `Producer` beans and producers as dependencies.

use std::any::{type_name, TypeId};

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Producer};
use r2e_core::type_list::TNil;

// ── Producer tests ────────────────────────────────────────────────────

#[derive(Clone)]
struct DbPool {
    url: String,
}

struct CreateDbPool;

impl Producer for CreateDbPool {
    type Output = DbPool;
    type Deps = TNil;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }

    async fn produce(_ctx: &BeanContext) -> DbPool {
        // Simulate async pool creation
        tokio::task::yield_now().await;
        DbPool {
            url: "sqlite::memory:".to_string(),
        }
    }
}

#[r2e_core::test]
async fn producer_resolution() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateDbPool>();
    let ctx = reg.resolve().await.unwrap();

    let pool: DbPool = ctx.get();
    assert_eq!(pool.url, "sqlite::memory:");
}

#[r2e_core::test]
async fn producer_as_dependency() {
    // Producer creates DbPool, then a sync bean depends on it.
    #[derive(Clone)]
    struct RepoService {
        pool: DbPool,
    }

    impl Bean for RepoService {
        type Deps = TNil;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<DbPool>(), type_name::<DbPool>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            Self {
                pool: ctx.get::<DbPool>(),
            }
        }
    }

    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateDbPool>();
    reg.register::<RepoService>();
    let ctx = reg.resolve().await.unwrap();

    let repo: RepoService = ctx.get();
    assert_eq!(repo.pool.url, "sqlite::memory:");
}
