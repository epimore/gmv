use uuid::Uuid;

use super::error::{GuardError, GuardResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Session,
    Stream,
    Avai,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeIdentity {
    pub node_id: String,
    pub instance_id: String,
    pub kind: NodeKind,
}

impl NodeIdentity {
    pub fn new(node_id: impl Into<String>, instance_id: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            node_id: node_id.into(),
            instance_id: instance_id.into(),
            kind,
        }
    }

    pub fn validate(&self) -> GuardResult<()> {
        validate_token(&self.node_id, "node_id")?;
        validate_token(&self.instance_id, "instance_id")
    }
}

pub fn generate_instance_id() -> String {
    Uuid::now_v7().to_string()
}

fn validate_token(value: &str, name: &str) -> GuardResult<()> {
    if value.is_empty() || value.len() > 128 {
        return Err(GuardError::InvalidIdentity(format!(
            "{name} must be 1..=128 chars"
        )));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(GuardError::InvalidIdentity(format!(
            "{name} must not contain whitespace"
        )));
    }
    Ok(())
}
