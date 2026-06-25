use guard::api::v2::ApiV2;
use guard::app_config::{GuardAppConfig, config_path_from_args};
use guard::job::SystemJobService;
use guard::operation::OperationService;
use guard::runtime::web::{self, WebServerConfig};
use guard::sim::{EndpointMode, Simulator};
use guard::store::InMemoryGuardStore;
use guard::store::persistent::PersistentStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = GuardAppConfig::load(config_path_from_args()?);
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let web_config = WebServerConfig::from_app(&config)?;
        let persistent = PersistentStore::connect(&config).await?;
        persistent.initialize(&config).await?;
        let users = persistent.load_users().await?;
        let store = InMemoryGuardStore::default();
        let simulator = if web_config.simulator_enabled {
            let simulator = Simulator::new(store.clone(), EndpointMode::Single);
            simulator.bootstrap(0)?;
            Some(simulator)
        } else {
            None
        };
        let api = ApiV2::new(
            store,
            OperationService::default(),
            SystemJobService::default(),
        );
        web::serve(
            web_config,
            api,
            persistent.outbox_repository(),
            simulator,
            users,
        )
        .await
    })
}
