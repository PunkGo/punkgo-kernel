//! Sandbox resource-limit configuration.
//!
//! Loaded from the `[sandbox]` section of `stellar.toml` (or a standalone
//! TOML file). All limits default to 0 = **unlimited** — the operator
//! opts in to enforcement via config, following the design principle that
//! the kernel is a committer, not a dictator.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use punkgo_core::errors::{KernelError, KernelResult};

/// Sandbox resource-limit configuration.
///
/// Every numeric field defaults to 0, meaning "unlimited / not enforced".
/// The operator enables limits by setting positive values in `stellar.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Max bytes for stdout **and** stderr (each). 0 = unlimited.
    #[serde(default)]
    pub output_max_bytes: u64,

    /// Extra filesystem paths beyond the actor workspace that the process
    /// is allowed to access. Empty = workspace only.
    /// (ProcessBackend records these for audit; true enforcement requires
    /// a container/VM backend.)
    #[serde(default)]
    pub filesystem_allowlist: Vec<PathBuf>,
}

/// Wrapper for TOML files that may contain a `[sandbox]` section.
#[derive(Debug, Deserialize)]
struct StellarWithSandbox {
    #[serde(default)]
    sandbox: SandboxConfig,
}

/// Load sandbox configuration from the same `stellar.toml` file that
/// holds stellar luminosity config.
///
/// If the file is missing or has no `[sandbox]` section, returns defaults
/// (all limits = 0 = unlimited).
pub fn load_sandbox_config(stellar_toml_path: &Path) -> KernelResult<SandboxConfig> {
    if !stellar_toml_path.exists() {
        return Ok(SandboxConfig::default());
    }

    let contents = std::fs::read_to_string(stellar_toml_path).map_err(|e| {
        KernelError::PolicyViolation(format!(
            "failed to read sandbox config from {}: {e}",
            stellar_toml_path.display()
        ))
    })?;

    let wrapper: StellarWithSandbox = toml::from_str(&contents).map_err(|e| {
        KernelError::PolicyViolation(format!(
            "failed to parse sandbox config from {}: {e}",
            stellar_toml_path.display()
        ))
    })?;

    Ok(wrapper.sandbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_all_unlimited() {
        let config = SandboxConfig::default();
        assert_eq!(config.output_max_bytes, 0);
        assert!(config.filesystem_allowlist.is_empty());
    }

    #[test]
    fn toml_roundtrip() {
        let config = SandboxConfig {
            output_max_bytes: 1_048_576,
            filesystem_allowlist: vec![PathBuf::from("/tmp/allowed")],
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: SandboxConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.output_max_bytes, 1_048_576);
        assert_eq!(parsed.filesystem_allowlist.len(), 1);
    }

    #[test]
    fn parse_empty_toml_returns_defaults() {
        let parsed: SandboxConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.output_max_bytes, 0);
    }

    #[test]
    fn parse_partial_toml() {
        let parsed: SandboxConfig = toml::from_str("output_max_bytes = 4096").unwrap();
        assert_eq!(parsed.output_max_bytes, 4096);
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let path = PathBuf::from("/nonexistent/stellar.toml");
        let config = load_sandbox_config(&path).unwrap();
        assert_eq!(config.output_max_bytes, 0);
    }
}
