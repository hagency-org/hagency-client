//! Room IDs are opaque; v12 hashes do not carry a server-name suffix.
pub(super) fn room_id(value: &str) -> bool {
    value.len() <= 255
        && value.starts_with('!')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && (value[1..]
            .split_once(':')
            .is_some_and(|(local, server)| !local.is_empty() && !server.is_empty())
            || (value.len() == 44
                && value[1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version12_rooms_keep_exact_identity_and_reject_truncated_or_foreign_sigil() {
        for room in [
            "!xBwLsfb3tVTO0jgpqkLGnxHIT4qWA6yILBlXdpHMSmY",
            "!legacy:test",
        ] {
            assert!(room_id(room));
        }
        for invalid in [
            "",
            "!",
            "!truncated",
            "@xBwLsfb3tVTO0jgpqkLGnxHIT4qWA6yILBlXdpHMSmY",
            "!:test",
            "!local:",
            "!has space:test",
            "!line\n:test",
        ] {
            assert!(!room_id(invalid));
        }
    }
}
