use std::sync::atomic::{AtomicUsize, Ordering};

use r2e_test::{TestApp, TestJwt};

static BEFORE_ALL: AtomicUsize = AtomicUsize::new(0);
static BEFORE_EACH: AtomicUsize = AtomicUsize::new(0);
static AFTER_EACH: AtomicUsize = AtomicUsize::new(0);
static AFTER_ALL: AtomicUsize = AtomicUsize::new(0);
static CASES: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct LifecycleSuite {
    local_cases: usize,
}

#[r2e::test_suite(tracing = false)]
impl LifecycleSuite {
    #[before_all]
    async fn setup() -> Self {
        BEFORE_ALL.fetch_add(1, Ordering::SeqCst);
        Self::default()
    }

    #[before_each]
    fn reset(&mut self) {
        assert_eq!(BEFORE_ALL.load(Ordering::SeqCst), 1);
        BEFORE_EACH.fetch_add(1, Ordering::SeqCst);
    }

    #[case]
    async fn first_case(&mut self) {
        self.local_cases += 1;
        CASES.fetch_add(1, Ordering::SeqCst);
    }

    #[case]
    fn second_case(&mut self) -> Result<(), &'static str> {
        self.local_cases += 1;
        CASES.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[after_each]
    fn cleanup(&mut self) {
        assert!(self.local_cases <= 2);
        AFTER_EACH.fetch_add(1, Ordering::SeqCst);
    }

    #[after_all]
    fn teardown(&mut self) {
        assert_eq!(self.local_cases, 2);
        assert_eq!(CASES.load(Ordering::SeqCst), 2);
        assert_eq!(BEFORE_EACH.load(Ordering::SeqCst), 2);
        assert_eq!(AFTER_EACH.load(Ordering::SeqCst), 2);
        AFTER_ALL.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SuiteGreeter(&'static str);

struct SuiteApp;

impl r2e::App for SuiteApp {
    type Env = ();

    async fn setup() -> Result<(), r2e::BootError> {
        Ok(())
    }

    async fn build(b: r2e::AppBuilder, _env: ()) -> Result<impl r2e::BootableApp, r2e::BootError> {
        Ok(b.provide(SuiteGreeter("real")).try_build_state().await?)
    }
}

struct AppBackedSuite {
    app: TestApp,
    greeter: SuiteGreeter,
    jwt_seen: bool,
}

#[r2e::test_suite(
    app = SuiteApp,
    tracing = false,
    with = |b| b.override_bean(SuiteGreeter("mock"))
)]
impl AppBackedSuite {
    #[before_all]
    async fn setup(app: TestApp, #[inject] greeter: SuiteGreeter, jwt: TestJwt) -> Self {
        let token = jwt.token("alice", &["user"]);
        assert_eq!(token.matches('.').count(), 2);
        Self {
            app,
            greeter,
            jwt_seen: true,
        }
    }

    #[case]
    fn sees_booted_app_scope(&mut self) {
        assert_eq!(self.app.bean::<SuiteGreeter>(), SuiteGreeter("mock"));
        assert_eq!(self.greeter, SuiteGreeter("mock"));
        assert!(self.jwt_seen);
    }
}
