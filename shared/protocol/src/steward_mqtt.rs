pub const DEFAULT_TOPIC_PREFIX: &str = "gmvc/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StewardTopics {
    prefix: String,
    installation_id: String,
}

impl StewardTopics {
    pub fn new(prefix: &str, installation_id: &str) -> Result<Self, &'static str> {
        let prefix = prefix.trim_matches('/');
        if !valid_topic_part(prefix, true) {
            return Err("invalid MQTT topic prefix");
        }
        if !valid_topic_part(installation_id, false) {
            return Err("invalid installation_id for MQTT topic");
        }
        Ok(Self {
            prefix: prefix.to_string(),
            installation_id: installation_id.to_string(),
        })
    }

    pub fn hello(&self) -> String {
        self.upstream("hello")
    }

    pub fn heartbeat(&self) -> String {
        self.upstream("heartbeat")
    }

    pub fn inventory(&self) -> String {
        self.upstream("inventory")
    }

    pub fn receipt(&self) -> String {
        self.upstream("receipt")
    }

    pub fn upgrade_request(&self) -> String {
        self.upstream("upgrade-request")
    }

    pub fn presence(&self) -> String {
        self.upstream("presence")
    }

    pub fn desired(&self) -> String {
        self.downstream("desired")
    }

    pub fn command(&self) -> String {
        self.downstream("command")
    }

    pub fn receipt_ack(&self) -> String {
        self.downstream("receipt-ack")
    }

    pub fn upgrade_decision(&self) -> String {
        self.downstream("upgrade-decision")
    }

    pub fn downstream_filter(&self) -> String {
        format!(
            "{}/installations/{}/down/#",
            self.prefix, self.installation_id
        )
    }

    fn upstream(&self, suffix: &str) -> String {
        format!(
            "{}/installations/{}/up/{suffix}",
            self.prefix, self.installation_id
        )
    }

    fn downstream(&self, suffix: &str) -> String {
        format!(
            "{}/installations/{}/down/{suffix}",
            self.prefix, self.installation_id
        )
    }
}

pub fn center_upstream_filter(prefix: &str) -> Result<String, &'static str> {
    let prefix = prefix.trim_matches('/');
    if !valid_topic_part(prefix, true) {
        return Err("invalid MQTT topic prefix");
    }
    Ok(format!("{prefix}/installations/+/up/#"))
}

pub fn installation_id_from_upstream_topic<'a>(prefix: &str, topic: &'a str) -> Option<&'a str> {
    let prefix = prefix.trim_matches('/');
    let rest = topic
        .strip_prefix(prefix)?
        .strip_prefix("/installations/")?;
    let (installation_id, suffix) = rest.split_once("/up/")?;
    if valid_topic_part(installation_id, false) && !suffix.is_empty() {
        Some(installation_id)
    } else {
        None
    }
}

fn valid_topic_part(value: &str, allow_slash: bool) -> bool {
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 128
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if allow_slash {
        value.split('/').all(valid_segment)
    } else {
        valid_segment(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_stable_device_topics() {
        let topics = StewardTopics::new("/gmvc/v1/", "site-01").unwrap();
        assert_eq!(topics.hello(), "gmvc/v1/installations/site-01/up/hello");
        assert_eq!(
            topics.desired(),
            "gmvc/v1/installations/site-01/down/desired"
        );
        assert_eq!(
            topics.downstream_filter(),
            "gmvc/v1/installations/site-01/down/#"
        );
        assert_eq!(
            installation_id_from_upstream_topic(
                DEFAULT_TOPIC_PREFIX,
                "gmvc/v1/installations/site-01/up/receipt"
            ),
            Some("site-01")
        );
    }

    #[test]
    fn rejects_wildcards_in_owned_topics() {
        assert!(StewardTopics::new("gmvc/+", "site-01").is_err());
        assert!(StewardTopics::new(DEFAULT_TOPIC_PREFIX, "site/#").is_err());
        assert!(StewardTopics::new(DEFAULT_TOPIC_PREFIX, "site 01").is_err());
    }
}
