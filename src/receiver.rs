/// Convert UUID string to 64-bit receiverId (first 16 hex nibbles → u64).
/// Matches readsb's read_uuid behavior.
pub fn uuid_to_receiver_id(uuid: &str) -> u64 {
    let hex: String = uuid.chars().filter(|c| *c != '-').take(16).collect();
    if hex.len() < 16 { return 0; }
    u64::from_str_radix(&hex, 16).unwrap_or(0)
}

pub fn is_valid_uuid(s: &str) -> bool {
    if s.len() != 36 { return false; }
    let stripped: String = s.chars().filter(|c| *c != '-').collect();
    stripped.len() == 32 && stripped.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_to_receiver_id() {
        let id = uuid_to_receiver_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert_eq!(id, 0xa1b2c3d4e5f67890);
    }

    #[test]
    fn test_valid_uuid() {
        assert!(is_valid_uuid("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(!is_valid_uuid("not-a-uuid"));
        assert!(!is_valid_uuid("a1b2c3d4e5f67890abcdef1234567890xxxx"));
    }
}
