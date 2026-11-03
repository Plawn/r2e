//! One type, two transports: a struct-level identity, an impl-level
//! `#[guard]` and `#[roles]` shared by the HTTP routes and the MCP members,
//! registered with both `register_controller` and `register_mcp_service`.

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;
use r2e::{Guard, GuardContext};

#[derive(Clone)]
pub struct CalcService;

impl CalcService {
    fn add(&self, a: f64, b: f64) -> f64 {
        a + b
    }
}

pub struct AlwaysOk;

impl r2e::SelfBuilt for AlwaysOk {}

impl<I: Identity> Guard<I> for AlwaysOk {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Rejection>> + Send {
        async { Ok(()) }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema, ObjectParams)]
pub struct Operands {
    pub a: f64,
    pub b: f64,
}

#[controller(path = "/calc")]
pub struct Calc {
    #[inject]
    calc: CalcService,
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
#[guard(AlwaysOk)]
#[roles("calc")]
impl Calc {
    #[get("/add/{a}/{b}")]
    async fn add_route(&self, Path((a, b)): Path<(f64, f64)>) -> String {
        format!("{} for {}", self.calc.add(a, b), self.user.sub)
    }

    #[tool]
    async fn add(&self, Params(p): Params<Operands>) -> String {
        format!("{} for {}", self.calc.add(p.a, p.b), self.user.sub)
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .provide(std::sync::Arc::new(r2e_test::TestJwt::new().claims_validator()))
            .provide(CalcService)
            .plugin(r2e::r2e_mcp::McpServer::new())
            .build_state()
            .await
            .register_controller::<Calc>()
            .register_mcp_service::<Calc>()
    };
}
