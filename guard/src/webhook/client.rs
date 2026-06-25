use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base::futures::StreamExt;
use reqwest::redirect::Policy;
use url::Url;

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::outbox::OutboxDelivery;
use crate::store::model::{OutboxDestinationKind, OutboxRecord};
use crate::webhook::policy::WebhookUrlPolicy;
use crate::webhook::signing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct WebhookClient {
    secret: Secret,
    timeout: Duration,
    max_response_bytes: usize,
    policy: WebhookUrlPolicy,
}

impl WebhookClient {
    pub fn new(
        secret: impl Into<String>,
        timeout: Duration,
        max_response_bytes: usize,
        policy: WebhookUrlPolicy,
    ) -> GuardResult<Self> {
        if timeout.is_zero() || max_response_bytes == 0 {
            return Err(GuardError::InvalidConfig(
                "webhook timeout and response limit must be positive".to_string(),
            ));
        }
        Ok(Self {
            secret: Secret::new(secret),
            timeout,
            max_response_bytes,
            policy,
        })
    }

    pub async fn send(&self, destination: &str, payload: &[u8]) -> GuardResult<WebhookResponse> {
        let url = Url::parse(destination)
            .map_err(|error| GuardError::InvalidConfig(format!("invalid webhook URL: {error}")))?;
        let addresses = self.policy.resolve(&url).await?;
        let host = url.host_str().expect("validated webhook host");
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(Policy::none())
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|error| GuardError::InvalidConfig(format!("webhook client: {error}")))?;
        let timestamp_ms = now_ms()?;
        let signature = signing::sign(self.secret.expose().as_bytes(), timestamp_ms, payload)?;
        let response = client
            .post(url)
            .header("content-type", "application/json")
            .header("x-gmv-timestamp", timestamp_ms.to_string())
            .header("x-gmv-signature", signature)
            .body(payload.to_vec())
            .send()
            .await
            .map_err(|error| GuardError::Conflict(format!("webhook request failed: {error}")))?;
        let status = response.status();
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                GuardError::Conflict(format!("webhook response failed: {error}"))
            })?;
            let remaining = self.max_response_bytes.saturating_sub(body.len());
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
            if body.len() == self.max_response_bytes {
                truncated = stream.next().await.is_some();
                break;
            }
        }
        let result = WebhookResponse {
            status: status.as_u16(),
            body,
            truncated,
        };
        if !status.is_success() {
            return Err(GuardError::Conflict(format!(
                "webhook returned HTTP {}: {}",
                result.status,
                String::from_utf8_lossy(&result.body)
            )));
        }
        Ok(result)
    }
}

impl OutboxDelivery for WebhookClient {
    fn deliver<'a>(
        &'a self,
        record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if record.destination_kind != OutboxDestinationKind::Webhook {
                return Err(GuardError::InvalidConfig(
                    "webhook client received non-webhook outbox record".to_string(),
                ));
            }
            self.send(&record.destination, &record.payload).await?;
            Ok(())
        })
    }
}

fn now_ms() -> GuardResult<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))
}
