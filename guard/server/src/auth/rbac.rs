#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

impl Role {
    pub fn allows(self, required: Role) -> bool {
        self >= required
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> crate::core::GuardResult<Self> {
        match value {
            "viewer" => Ok(Self::Viewer),
            "operator" => Ok(Self::Operator),
            "admin" => Ok(Self::Admin),
            _ => Err(crate::core::GuardError::InvalidConfig(format!(
                "invalid UI role {value}"
            ))),
        }
    }
}
