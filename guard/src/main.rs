use guard::api::v2::ApiV2;
use guard::app_config::{GuardAppConfig, config_path_from_args};
use guard::job::SystemJobService;
use guard::operation::OperationService;
use guard::runtime::node_rpc::{self, NodeRpcConfig};
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
        let user_repository = persistent.user_repository();
        let store = InMemoryGuardStore::default();
        let registry = guard::registry::RegistryService::new(store.clone());
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
        let rpc_config = NodeRpcConfig {
            bind_addr: config.grpc.bind_addr,
            heartbeat_interval_ms: config.grpc.heartbeat_interval_ms,
            heartbeat_timeout_ms: config.grpc.heartbeat_timeout_ms,
        };
        let web = web::serve(
            web_config,
            api,
            persistent.outbox_repository(),
            simulator,
            users,
            user_repository,
        );
        let rpc = node_rpc::serve(rpc_config, registry);
        base::tokio::try_join!(web, rpc).map(|_| ())
    })
}
