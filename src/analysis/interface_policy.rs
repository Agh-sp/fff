use anyhow::{Context, Result};
use log::warn;

/// A failed optional module contributes no entries, not a partial interface map.
/// Keep this exception narrow: failures in game modules (and unknown modules)
/// must still abort analysis instead of silently dropping required data.
pub(super) fn module_result<T>(module_name: &str, result: Result<T>) -> Result<Option<T>> {
    match result.with_context(|| format!("failed to read interfaces in {module_name}")) {
        Ok(interfaces) => Ok(Some(interfaces)),
        Err(err) if module_name.eq_ignore_ascii_case("vstdlib_s64.dll") => {
            // Steam's support DLL need not use the same CreateInterface layout
            // as Source 2 modules. Its interfaces aren't required for the game
            // offsets/schemas, but try to retain them when extraction succeeds.
            warn!(
                "skipping optional interface module {module_name}; its interfaces are omitted from this dump: {err:#}"
            );
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::collections::BTreeMap;

    #[test]
    fn steam_partial_read_does_not_abort_analysis() {
        let result = module_result::<()>("vstdlib_s64.dll", Err(anyhow!("memory: partial data")));
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn optional_module_matching_is_case_insensitive_but_exact() {
        assert!(
            module_result::<()>("VSTDLIB_S64.DLL", Err(anyhow!("read failed")))
                .unwrap()
                .is_none()
        );
        for name in [
            "vstdlib.dll",
            "other_vstdlib_s64.dll",
            "vstdlib_s64.dll.bak",
        ] {
            assert!(module_result::<()>(name, Err(anyhow!("read failed"))).is_err());
        }
    }

    #[test]
    fn required_and_unknown_modules_keep_error_context() {
        for name in [
            "client.dll",
            "engine2.dll",
            "schemasystem.dll",
            "tier0.dll",
            "unknown.dll",
        ] {
            let err = module_result::<()>(name, Err(anyhow!("memory: partial data"))).unwrap_err();
            let message = format!("{err:#}");
            assert!(message.contains(name));
            assert!(message.contains("memory: partial data"));
        }
    }

    #[test]
    fn preserves_successful_optional_interfaces() {
        let entries = BTreeMap::from([("SteamInterface".to_string(), 0x100_u64)]);
        let result = module_result("vstdlib_s64.dll", Ok(entries.clone())).unwrap();
        assert_eq!(result, Some(entries));
    }

    #[test]
    fn continues_to_next_module_after_optional_failure() {
        let mut collected = BTreeMap::new();
        for (name, result) in [
            ("engine2.dll", Ok(1)),
            ("vstdlib_s64.dll", Err(anyhow!("memory: partial data"))),
            ("client.dll", Ok(2)),
        ] {
            if let Some(value) = module_result(name, result).unwrap() {
                collected.insert(name, value);
            }
        }
        assert_eq!(
            collected,
            BTreeMap::from([("engine2.dll", 1), ("client.dll", 2)])
        );
    }
}
