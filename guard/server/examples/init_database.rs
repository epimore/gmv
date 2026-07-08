use gmv_guard_server::app_config::{GuardAppConfig, config_path_from_args};
use gmv_guard_server::store::persistent::PersistentStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = GuardAppConfig::load(config_path_from_args()?);
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let store = PersistentStore::connect(&config).await?;
        store.migrate().await?;
        store.initialize(&config).await?;
        Ok::<(), gmv_guard_server::core::GuardError>(())
    })?;
    println!("guard database initialized");
    Ok(())
}
