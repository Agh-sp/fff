use std::collections::HashSet;
use std::ffi::c_char;

use anyhow::{Result, bail, ensure};

// Defensive limits for counts read from a live, potentially changing process.
pub const MAX_REMOTE_ELEMENTS: usize = 1_000_000;

pub fn checked_count(count: i32) -> Result<usize> {
    let count = usize::try_from(count)?;
    ensure!(
        count <= MAX_REMOTE_ELEMENTS,
        "remote element count exceeds safety limit"
    );
    Ok(count)
}

pub fn check_index(count: i32, index: usize) -> bool {
    checked_count(count).is_ok_and(|count| index < count)
}

pub fn bounded_name(bytes: &[c_char]) -> Result<String> {
    let Some(end) = bytes.iter().position(|&byte| byte == 0) else {
        bail!("unterminated schema scope name");
    };
    ensure!(end != 0, "empty schema scope name");
    let bytes: Vec<u8> = bytes[..end].iter().map(|&byte| byte as u8).collect();
    Ok(String::from_utf8(bytes)?)
}

#[derive(Default)]
pub struct TraversalGuard {
    seen: HashSet<u64>,
}

impl TraversalGuard {
    pub fn visit(&mut self, address: u64) -> Result<()> {
        ensure!(
            self.seen.len() < MAX_REMOTE_ELEMENTS,
            "remote list exceeds safety limit"
        );
        ensure!(
            self.seen.insert(address),
            "cycle in remote list at {address:#X}"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_counts_and_indices() {
        assert!(checked_count(-1).is_err());
        assert!(checked_count(i32::MAX).is_err());
        assert_eq!(checked_count(0).unwrap(), 0);
        assert!(!check_index(-1, 0));
        assert!(!check_index(0, 0));
        assert!(check_index(2, 1));
        assert!(!check_index(2, 2));
        assert!(!check_index(2, usize::MAX));
    }

    #[test]
    fn bounds_scope_names() {
        assert_eq!(
            bounded_name(&[b'a' as c_char, 0, b'b' as c_char]).unwrap(),
            "a"
        );
        assert!(bounded_name(&[b'a' as c_char; 256]).is_err());
        assert!(bounded_name(&[0; 256]).is_err());
        assert!(bounded_name(&[-1_i8 as c_char, 0]).is_err());
    }

    #[test]
    fn rejects_cycles() {
        let mut guard = TraversalGuard::default();
        guard.visit(1).unwrap();
        guard.visit(2).unwrap();
        assert!(guard.visit(1).is_err());
    }
}
