pub const DEFAULT_TOPIC_PREFIX: &str = "gmvc/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmvCenterAgentTopics {
    prefix: String,
    installation_id: String,
    host_id: String,
}

impl GmvCenterAgentTopics {
    pub fn new(prefix: &str, installation_id: &str, host_id: &str) -> Result<Self, &'static str> {
        let prefix = prefix.trim_matches('/');
        if !valid_topic_part(prefix, true) {
            return Err("invalid MQTT topic prefix");
        }
        if !valid_topic_part(installation_id, false) {
            return Err("invalid installation_id for MQTT topic");
        }
        if !valid_topic_part(host_id, false) {
            return Err("invalid host_id for MQTT topic");
        }
        Ok(Self {
            prefix: prefix.to_string(),
            installation_id: installation_id.to_string(),
            host_id: host_id.to_string(),
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

    pub fn component_desired(&self, component_id: &str) -> Result<String, &'static str> {
        if !valid_topic_part(component_id, false) {
            return Err("invalid component_id for MQTT topic");
        }
        Ok(self.downstream(&format!("components/{component_id}/desired")))
    }

    pub fn component_id_from_desired_topic<'a>(&self, topic: &'a str) -> Option<&'a str> {
        let prefix = format!(
            "{}/installations/{}/hosts/{}/down/components/",
            self.prefix, self.installation_id, self.host_id
        );
        let component_id = topic.strip_prefix(&prefix)?.strip_suffix("/desired")?;
        valid_topic_part(component_id, false).then_some(component_id)
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
            "{}/installations/{}/hosts/{}/down/#",
            self.prefix, self.installation_id, self.host_id
        )
    }

    fn upstream(&self, suffix: &str) -> String {
        format!(
            "{}/installations/{}/hosts/{}/up/{suffix}",
            self.prefix, self.installation_id, self.host_id
        )
    }

    fn downstream(&self, suffix: &str) -> String {
        format!(
            "{}/installations/{}/hosts/{}/down/{suffix}",
            self.prefix, self.installation_id, self.host_id
        )
    }
}

pub fn center_upstream_filter(prefix: &str) -> Result<String, &'static str> {
    let prefix = prefix.trim_matches('/');
    if !valid_topic_part(prefix, true) {
        return Err("invalid MQTT topic prefix");
    }
    Ok(format!("{prefix}/installations/+/hosts/+/up/#"))
}

pub fn identity_from_upstream_topic<'a>(
    prefix: &str,
    topic: &'a str,
) -> Option<(&'a str, &'a str)> {
    let prefix = prefix.trim_matches('/');
    let rest = topic
        .strip_prefix(prefix)?
        .strip_prefix("/installations/")?;
    let (installation_id, rest) = rest.split_once("/hosts/")?;
    let (host_id, suffix) = rest.split_once("/up/")?;
    if valid_topic_part(installation_id, false)
        && valid_topic_part(host_id, false)
        && !suffix.is_empty()
    {
        Some((installation_id, host_id))
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
        let topics = GmvCenterAgentTopics::new("/gmvc/v1/", "site-01", "host-01").unwrap();
        assert_eq!(
            topics.hello(),
            "gmvc/v1/installations/site-01/hosts/host-01/up/hello"
        );
        assert_eq!(
            topics.desired(),
            "gmvc/v1/installations/site-01/hosts/host-01/down/desired"
        );
        assert_eq!(
            topics.component_desired("guard").unwrap(),
            "gmvc/v1/installations/site-01/hosts/host-01/down/components/guard/desired"
        );
        assert_eq!(
            topics.component_desired("stream").unwrap(),
            "gmvc/v1/installations/site-01/hosts/host-01/down/components/stream/desired"
        );
        assert_eq!(
            topics.component_id_from_desired_topic(
                "gmvc/v1/installations/site-01/hosts/host-01/down/components/guard/desired"
            ),
            Some("guard")
        );
        assert_eq!(
            topics.downstream_filter(),
            "gmvc/v1/installations/site-01/hosts/host-01/down/#"
        );
        assert_eq!(
            identity_from_upstream_topic(
                DEFAULT_TOPIC_PREFIX,
                "gmvc/v1/installations/site-01/hosts/host-01/up/receipt"
            ),
            Some(("site-01", "host-01"))
        );
    }

    #[test]
    fn rejects_wildcards_in_owned_topics() {
        assert!(GmvCenterAgentTopics::new("gmvc/+", "site-01", "host-01").is_err());
        assert!(GmvCenterAgentTopics::new(DEFAULT_TOPIC_PREFIX, "site/#", "host-01").is_err());
        assert!(GmvCenterAgentTopics::new(DEFAULT_TOPIC_PREFIX, "site-01", "host/#").is_err());
        let topics = GmvCenterAgentTopics::new(DEFAULT_TOPIC_PREFIX, "site-01", "host-01").unwrap();
        assert!(topics.component_desired("stream/#").is_err());
        assert!(
            topics
                .component_id_from_desired_topic(
                    "gmvc/v1/installations/site-01/hosts/host-01/down/components/guard/other"
                )
                .is_none()
        );
    }
}
