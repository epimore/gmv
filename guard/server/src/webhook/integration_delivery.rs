use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::core::{GuardError, GuardResult};
use crate::integration::model::{CredentialPurpose, IntegrationTransport};
use crate::integration::secret::IntegrationSecretManager;
use crate::outbox::OutboxDelivery;
use crate::store::model::{OutboxDestinationKind, OutboxRecord};
use crate::store::persistent::IntegrationRepository;
use crate::webhook::{WebhookClient, WebhookUrlPolicy};

#[derive(Debug, Clone)]
pub struct IntegrationWebhookDelivery {
    integrations: IntegrationRepository,
    secrets: IntegrationSecretManager,
}

impl IntegrationWebhookDelivery {
    pub fn new(integrations: IntegrationRepository, secrets: IntegrationSecretManager) -> Self {
        Self {
            integrations,
            secrets,
        }
    }

    async fn deliver_record(&self, record: &OutboxRecord) -> GuardResult<()> {
        if record.destination_kind != OutboxDestinationKind::Webhook {
            return Err(GuardError::InvalidConfig(
                "integration webhook received non-HTTP outbox record".to_string(),
            ));
        }
        let now_ms = now_ms();
        if self
            .integrations
            .business_integration_id()
            .await?
            .as_deref()
            != Some(record.integration_id.as_str())
        {
            return Err(GuardError::InvalidIdentity(
                "HTTP integration is not the active business integration".to_string(),
            ));
        }
        let integration = self
            .integrations
            .get(&record.integration_id)
            .await?
            .filter(|value| {
                value.transport == IntegrationTransport::Http
                    && value.enabled
                    && value.outbound_enabled
                    && value
                        .expires_at_ms
                        .is_none_or(|expires_at| expires_at > now_ms)
            })
            .ok_or_else(|| {
                GuardError::InvalidIdentity("HTTP integration is disabled".to_string())
            })?;
        let config = self
            .integrations
            .http_config(&integration.integration_id)
            .await?
            .ok_or_else(|| {
                GuardError::InvalidConfig("HTTP integration config missing".to_string())
            })?;
        let credential = self
            .integrations
            .list_credentials(&integration.integration_id)
            .await?
            .into_iter()
            .find(|credential| {
                credential.purpose == CredentialPurpose::HttpCallbackSign
                    && credential.is_active_at(now_ms)
            })
            .ok_or_else(|| {
                GuardError::InvalidIdentity("HTTP callback credential missing".to_string())
            })?;
        let secret = self.secrets.decrypt(&credential.secret_ciphertext).await?;
        WebhookClient::new(
            secret,
            Duration::from_millis(u64::try_from(config.callback_timeout_ms).unwrap_or(5_000)),
            usize::try_from(config.max_response_bytes).unwrap_or(65_536),
            WebhookUrlPolicy {
                private_network_allowlist: config.private_network_allowlist,
            },
        )?
        .with_access_key(credential.access_key)
        .send(&record.destination, &record.payload)
        .await?;
        Ok(())
    }
}

impl OutboxDelivery for IntegrationWebhookDelivery {
    fn deliver<'a>(
        &'a self,
        record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async move { self.deliver_record(record).await })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
