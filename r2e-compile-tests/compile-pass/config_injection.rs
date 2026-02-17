use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub config: R2eConfig,
}

impl FromRef<AppState> for R2eConfig {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}

#[derive(Controller)]
#[controller(path = "/cfg", state = AppState)]
pub struct ConfigController {
    #[config("app.name")]
    name: String,
    #[config("app.debug")]
    debug: bool,
    #[config("app.optional")]
    optional: Option<String>,
}

#[routes]
impl ConfigController {
    #[get("/name")]
    async fn get_name(&self) -> String {
        self.name.clone()
    }
}

fn main() {}
