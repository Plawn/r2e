use std::sync::Arc;

use iggy::prelude::IggyClient;

use r2e_events::backend::BackendState;

use crate::config::IggyConfig;

/// Shared inner state for `IggyEventBus`, behind an `Arc`.
pub(crate) struct IggyInner {
    pub config: IggyConfig,
    pub client: Arc<IggyClient>,
    pub state: Arc<BackendState>,
}
