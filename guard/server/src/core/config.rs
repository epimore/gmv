use super::error::{GuardError, GuardResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardConfig {
    pub bind_host: String,
    pub tls: TlsConfig,
    pub heartbeat: HeartbeatConfig,
    pub bus: BusConfig,
    pub lease: LeaseConfig,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            bind_host: "127.0.0.1".to_string(),
            tls: TlsConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            bus: BusConfig::default(),
            lease: LeaseConfig::default(),
        }
    }
}

impl GuardConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if !self.tls.enabled && !is_loopback_host(&self.bind_host) {
            return Err(GuardError::InvalidConfig(
                "TLS can only be disabled on loopback development binds".to_string(),
            ));
        }
        self.heartbeat.validate()?;
        self.bus.validate()?;
        self.lease.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsConfig {
    pub enabled: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatConfig {
    pub interval_ms: u64,
    pub timeout_ms: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_ms: 5_000,
            timeout_ms: 15_000,
        }
    }
}

impl HeartbeatConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if self.interval_ms == 0 {
            return Err(GuardError::InvalidConfig(
                "heartbeat interval must be positive".to_string(),
            ));
        }
        if self.timeout_ms < self.interval_ms * 3 {
            return Err(GuardError::InvalidConfig(
                "heartbeat timeout must be at least three intervals".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusConfig {
    pub consumer_queue_capacity: usize,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            consumer_queue_capacity: 128,
        }
    }
}

impl BusConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if self.consumer_queue_capacity == 0 {
            return Err(GuardError::InvalidConfig(
                "bus consumer queue capacity must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseConfig {
    pub allocation_ttl_ms: u64,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            allocation_ttl_ms: 30_000,
        }
    }
}

impl LeaseConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if self.allocation_ttl_ms == 0 {
            return Err(GuardError::InvalidConfig(
                "allocation ttl must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_tls_enabled() {
        let config = GuardConfig::default();
        assert!(config.tls.enabled);
        config.validate().unwrap();
    }

    #[test]
    fn insecure_non_loopback_bind_is_rejected() {
        let mut config = GuardConfig::default();
        config.bind_host = "0.0.0.0".to_string();
        config.tls.enabled = false;
        assert!(matches!(
            config.validate(),
            Err(GuardError::InvalidConfig(_))
        ));
    }

    #[test]
    fn heartbeat_timeout_must_cover_three_intervals() {
        let config = HeartbeatConfig {
            interval_ms: 5_000,
            timeout_ms: 10_000,
        };
        assert!(config.validate().is_err());
    }
}
