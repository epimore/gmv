use guard::app_config::{GuardAppConfig, config_path_from_args};
use guard::store::persistent::PersistentStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = GuardAppConfig::load(config_path_from_args()?);
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let store = PersistentStore::connect(&config).await?;
        store.migrate().await?;
        store.initialize(&config).await?;
        Ok::<(), guard::core::GuardError>(())
    })?;
    println!("guard database initialized");
    Ok(())
}
