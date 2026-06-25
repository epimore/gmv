use crate::auth::Secret;
use crate::core::{GuardError, GuardResult, NodeKind};

#[derive(Debug, Clone)]
pub struct ServiceCredential {
    pub node_id: String,
    pub kind: NodeKind,
    token: Secret,
}

impl ServiceCredential {
    pub fn new(node_id: impl Into<String>, kind: NodeKind, token: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            kind,
            token: Secret::new(token),
        }
    }

    pub fn verify(&self, node_id: &str, kind: NodeKind, token: &str) -> GuardResult<()> {
        if self.node_id != node_id || self.kind != kind || !self.token.verify(token) {
            return Err(GuardError::InvalidIdentity(
                "service credential mismatch".to_string(),
            ));
        }
        Ok(())
    }
}
