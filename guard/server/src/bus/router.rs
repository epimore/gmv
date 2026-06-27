pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pattern_parts = pattern.split('.').collect::<Vec<_>>();
    let topic_parts = topic.split('.').collect::<Vec<_>>();
    matches_parts(&pattern_parts, &topic_parts)
}

fn matches_parts(pattern: &[&str], topic: &[&str]) -> bool {
    match (pattern.split_first(), topic.split_first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some((&"**", rest)), _) => {
            matches_parts(rest, topic)
                || topic
                    .split_first()
                    .is_some_and(|(_, tail)| matches_parts(pattern, tail))
        }
        (Some((&"*", rest)), Some((_, topic_rest))) => matches_parts(rest, topic_rest),
        (Some((head, rest)), Some((topic_head, topic_rest))) if head == topic_head => {
            matches_parts(rest, topic_rest)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_single_and_multi_level_wildcards() {
        assert!(topic_matches("node.*.health", "node.stream.health"));
        assert!(topic_matches("node.**", "node.stream.health.ready"));
        assert!(!topic_matches("node.*.health", "node.stream.health.ready"));
    }
}
