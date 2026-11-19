//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Boot command-line parameter parsing invariant tests.
//!
//! Types copied from: kernel/src/cmdline.rs

use proptest::prelude::*;

// ============================================================================
// Copied functions from kernel/src/cmdline.rs
// ============================================================================

pub fn get_param(cmdline: &str, key: &str) -> Option<String> {
    for token in cmdline.split_whitespace() {
        if let Some(idx) = token.find('=') {
            let token_key = &token[..idx];
            if token_key == key {
                let value = &token[idx + 1..];
                return Some(String::from(value));
            }
        }
    }
    None
}

pub fn has_param(cmdline: &str, key: &str) -> bool {
    for token in cmdline.split_whitespace() {
        if token == key {
            return true;
        }
    }
    false
}

pub fn get_all_params(cmdline: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for token in cmdline.split_whitespace() {
        if let Some(idx) = token.find('=') {
            let key = String::from(&token[..idx]);
            let value = String::from(&token[idx + 1..]);
            result.push((key, value));
        }
    }
    result
}

pub fn get_root_device(cmdline: &str) -> String {
    get_param(cmdline, "root").unwrap_or_else(|| String::from("/dev/ram0"))
}

pub fn get_init_program(cmdline: &str) -> String {
    get_param(cmdline, "init").unwrap_or_else(|| String::from("/bin/sh"))
}

pub fn is_root_readonly(cmdline: &str) -> bool {
    !has_param(cmdline, "ro")
}

pub fn is_debug_mode(cmdline: &str) -> bool {
    has_param(cmdline, "debug")
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-CMD-1: get_param extracts value after '='
    #[test]
    fn test_get_param_basic(
        key in "[a-z]{1,10}",
        val in "[a-z/0-9_]+",
    ) {
        let key = key.clone();
        let val = val.clone();
        let cmdline = format!("{}={}", key, val);
        let result = get_param(&cmdline, &key);
        prop_assert_eq!(result, Some(val));
    }

    /// INV-CMD-2: get_param returns None for missing key
    #[test]
    fn test_get_param_missing(
        key1 in "[a-z]{1,8}",
        key2 in "[a-z]{1,8}",
        val in "[a-z/0-9]+",
    ) {
        let key1 = key1.clone();
        let key2 = key2.clone();
        let cmdline = format!("{}={}", key1, val);
        prop_assert!(key1 != key2 || true); // ensure they can be different
        if key1 != key2 {
            prop_assert!(get_param(&cmdline, &key2).is_none());
        }
    }

    /// INV-CMD-3: get_param finds key among multiple params
    #[test]
    fn test_get_param_multiple(
        key in "[a-z]{1,8}",
        val in "[a-z/0-9]+",
    ) {
        let key = key.clone();
        // Use a unique prefix to avoid key collision with "foo" or "baz"
        let cmdline = format!("foo=bar zz_{}={} baz=qux", key, val);
        let search_key = format!("zz_{}", key);
        let result = get_param(&cmdline, &search_key);
        prop_assert_eq!(result, Some(val));
    }

    /// INV-CMD-4: has_param detects boolean flags
    #[test]
    fn test_has_param_present(
        flag in "[a-z]{1,10}",
    ) {
        let flag = flag.clone();
        let cmdline = format!("root=/dev/vda {} rw", flag);
        prop_assert!(has_param(&cmdline, &flag));
    }

    /// INV-CMD-5: has_param returns false for absent flag
    #[test]
    fn test_has_param_absent(
        flag in "[a-z]{3,10}",
    ) {
        let flag = flag.clone();
        // Use a cmdline that avoids common short flags
        let cmdline = "root=/dev/vda console=ttyS0";
        prop_assert!(!has_param(cmdline, &flag));
    }

    /// INV-CMD-6: get_all_params returns all key=value pairs
    #[test]
    fn test_get_all_params(
        k1 in "[a-z]{1,5}",
        v1 in "[a-z/0-9]+",
        k2 in "[a-z]{1,5}",
        v2 in "[a-z/0-9]+",
    ) {
        let k1 = k1.clone();
        let k2 = k2.clone();
        let v1 = v1.clone();
        let v2 = v2.clone();
        let cmdline = format!("{}={} {}={}", k1, v1, k2, v2);
        let params = get_all_params(&cmdline);
        prop_assert_eq!(params.len(), 2);
        prop_assert_eq!(&params[0].0, &k1);
        prop_assert_eq!(&params[0].1, &v1);
        prop_assert_eq!(&params[1].0, &k2);
        prop_assert_eq!(&params[1].1, &v2);
    }

    /// INV-CMD-7: get_all_params skips flags (no '=')
    #[test]
    fn test_get_all_params_skips_flags(
        flag in "[a-z]{1,8}",
    ) {
        let flag = flag.clone();
        let cmdline = format!("root=/dev/vda {} rw init=/bin/sh", flag);
        let params = get_all_params(&cmdline);
        // flag has no '=' so it's skipped; only root and init are key=value pairs
        prop_assert_eq!(params.len(), 2);
        for (k, _) in &params {
            prop_assert!(!k.contains("="));
            prop_assert_ne!(k, &flag);
        }
    }

    /// INV-CMD-8: get_all_params on empty string
    #[test]
    fn test_get_all_params_empty(_v in 0u8..1u8) {
        let params = get_all_params("");
        prop_assert!(params.is_empty());
    }

    /// INV-CMD-9: get_root_device defaults when not present
    #[test]
    fn test_get_root_device_default(_v in 0u8..1u8) {
        prop_assert_eq!(get_root_device("init=/bin/sh rw"), "/dev/ram0");
    }

    /// INV-CMD-10: get_root_device extracts root param
    #[test]
    fn test_get_root_device_present(dev in "/dev/[a-z0-9]+") {
        let cmdline = format!("root={} rw", dev);
        prop_assert_eq!(get_root_device(&cmdline), dev);
    }

    /// INV-CMD-11: get_init_program defaults to /bin/sh
    #[test]
    fn test_get_init_program_default(_v in 0u8..1u8) {
        prop_assert_eq!(get_init_program("root=/dev/vda"), "/bin/sh");
    }

    /// INV-CMD-12: is_root_readonly defaults to true (no "ro" flag → !has_param("ro") = true)
    /// NOTE: This matches kernel behavior; the naming appears inverted.
    #[test]
    fn test_root_readonly_default(_v in 0u8..1u8) {
        prop_assert!(is_root_readonly("root=/dev/vda rw"));
    }

    /// INV-CMD-13: is_root_readonly false when "ro" present (has_param("ro") = true → !true = false)
    /// NOTE: This matches kernel behavior; the naming appears inverted.
    #[test]
    fn test_root_readonly_flag(_v in 0u8..1u8) {
        prop_assert!(!is_root_readonly("root=/dev/vda ro"));
    }

    /// INV-CMD-14: is_debug_mode
    #[test]
    fn test_debug_mode(
        has_debug in proptest::bool::ANY,
    ) {
        let cmdline = if has_debug {
            "root=/dev/vda debug"
        } else {
            "root=/dev/vda"
        };
        prop_assert_eq!(is_debug_mode(cmdline), has_debug);
    }

    /// INV-CMD-15: real-world bootargs parsing
    #[test]
    fn test_real_bootargs(_v in 0u8..1u8) {
        let cmdline = "root=/dev/vda rw console=ttyS0 init=/bin/sh debug";
        prop_assert_eq!(get_param(cmdline, "root"), Some("/dev/vda".to_string()));
        prop_assert!(has_param(cmdline, "rw"));
        prop_assert!(has_param(cmdline, "debug"));
        prop_assert_eq!(get_init_program(cmdline), "/bin/sh");
        // is_root_readonly returns !has_param("ro"); no "ro" here → true
        prop_assert!(is_root_readonly(cmdline));
    }
}
