use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationPrincipal {
    pub integration_id: String,
    pub scope: String,
}

impl IntegrationPrincipal {
    pub fn identity(&self) -> String {
        format!("integration:{}", self.integration_id)
    }
}

base::tokio::task_local! {
    static CURRENT_INTEGRATION_PRINCIPAL: IntegrationPrincipal;
}

pub async fn scope<F>(principal: IntegrationPrincipal, future: F) -> F::Output
where
    F: Future,
{
    CURRENT_INTEGRATION_PRINCIPAL.scope(principal, future).await
}

pub fn current() -> Option<IntegrationPrincipal> {
    CURRENT_INTEGRATION_PRINCIPAL.try_with(Clone::clone).ok()
}
