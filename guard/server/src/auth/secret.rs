use std::fmt::{Debug, Display, Formatter};

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn verify(&self, candidate: &str) -> bool {
        self.0 == candidate
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for Secret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

impl Display for Secret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_does_not_display_value() {
        let secret = Secret::new("token-123");
        assert!(!format!("{secret:?}").contains("token-123"));
        assert!(!secret.to_string().contains("token-123"));
        assert!(secret.verify("token-123"));
    }
}
