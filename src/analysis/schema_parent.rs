//! Parent metadata is optional, but failed discovery must be distinguishable
//! from a class that explicitly declares no base class.
use anyhow::{Result, bail, ensure};

const MAX_PARENT_NAME_BYTES: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ParentInfo {
    pub name: Option<String>,
    pub error: Option<String>,
}

pub(super) fn resolve_parent(
    has_base_class: u8,
    read: impl FnOnce() -> Result<String>,
) -> ParentInfo {
    if has_base_class == 0 {
        // A non-null residual pointer is not proof of inheritance.
        return ParentInfo {
            name: None,
            error: None,
        };
    }

    match read().and_then(|name| {
        ensure!(!name.is_empty(), "invalid parent class name: empty string");
        Ok(name)
    }) {
        Ok(name) => ParentInfo {
            name: Some(name),
            error: None,
        },
        Err(err) => ParentInfo {
            name: None,
            error: Some(format!("{err:#}")),
        },
    }
}

/// Read only through the terminator, so a name at a page boundary does not
/// require bytes beyond its end. Unlike read_utf8_lossy, this cannot interpret
/// zero-filled partial reads as a genuine empty name.
pub(super) fn read_parent_name(mut read_byte: impl FnMut(usize) -> Result<u8>) -> Result<String> {
    let mut bytes = Vec::new();
    for index in 0..MAX_PARENT_NAME_BYTES {
        let byte = read_byte(index)?;
        if byte == 0 {
            ensure!(!bytes.is_empty(), "invalid parent class name: empty string");
            return Ok(String::from_utf8(bytes)?);
        }
        bytes.push(byte);
    }
    bail!("parent class name is not NUL-terminated within {MAX_PARENT_NAME_BYTES} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, anyhow};

    #[test]
    fn root_class_does_not_dereference_a_residual_pointer() {
        let parent = resolve_parent(0, || panic!("must not read a parent when flag is zero"));
        assert_eq!(
            parent,
            ParentInfo {
                name: None,
                error: None
            }
        );
    }

    #[test]
    fn declared_parent_is_preserved() {
        let parent = resolve_parent(1, || Ok("C_BaseEntity".into()));
        assert_eq!(
            parent,
            ParentInfo {
                name: Some("C_BaseEntity".into()),
                error: None
            }
        );
    }

    #[test]
    fn empty_parent_is_unknown_not_absent() {
        let parent = resolve_parent(1, || Ok(String::new()));
        assert!(parent.name.is_none());
        assert!(parent.error.unwrap().contains("empty string"));
    }

    #[test]
    fn failed_parent_read_keeps_the_error_chain() {
        let parent = resolve_parent(1, || {
            Err(anyhow!("memory: partial data")).context("failed to read parent binding")
        });
        assert!(parent.name.is_none());
        let message = parent.error.unwrap();
        assert!(message.contains("failed to read parent binding"));
        assert!(message.contains("memory: partial data"));
    }

    #[test]
    fn name_read_stops_at_terminator() {
        let name = b"CBase\0";
        assert_eq!(read_parent_name(|index| Ok(name[index])).unwrap(), "CBase");
    }

    #[test]
    fn names_are_not_silently_truncated_to_128_bytes() {
        let mut bytes = vec![b'A'; 200];
        bytes.push(0);
        assert_eq!(
            read_parent_name(|index| Ok(bytes[index])).unwrap().len(),
            200
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert!(read_parent_name(|_| Ok(0)).is_err());
        assert!(read_parent_name(|_| Ok(b'A')).is_err());
        assert!(read_parent_name(|index| Ok([0xFF, 0][index])).is_err());
    }

    #[test]
    fn interrupted_name_read_stays_an_error() {
        let err = read_parent_name(|index| {
            if index == 2 {
                Err(anyhow!("memory: partial data"))
            } else {
                Ok(b'A')
            }
        })
        .unwrap_err();
        assert!(err.to_string().contains("partial data"));
    }
}
