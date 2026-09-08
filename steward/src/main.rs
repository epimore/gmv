use base::utils::rt::{GlobalRuntime, RuntimeType};
use gmv_steward::{StewardConfig, run};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    base::daemon::install_sanitized_panic_hook();
    base::logger::Logger::init()?;
    let config = StewardConfig::load()?;
    let runtime = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
    let service_runtime = runtime.clone();
    runtime.spawn("steward-service", async move {
        if let Err(error) = run(config, service_runtime).await {
            base::log::error!("Steward runtime failed: {error}");
            GlobalRuntime::request_shutdown_with_error();
        }
    })?;
    let report = GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork]);
    if !report.is_graceful() {
        return Err(std::io::Error::other("Steward shutdown was incomplete").into());
    }
    Ok(())
}
