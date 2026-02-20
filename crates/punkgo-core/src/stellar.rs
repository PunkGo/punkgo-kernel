//! Stellar luminosity configuration — anchors energy production to hardware compute power.
//!
//! Covers: PIP-001 §1 (stellar luminosity), §4 (no-starvation minimum).
//!
//! The kernel determines an `energy_per_tick` value on startup based on the
//! machine's INT8 TOPS (tera-operations per second). This value sets the total
//! energy budget produced each tick and distributed among active actors.
//!
//! Configuration is loaded from a TOML file (`stellar.toml`) in the state
//! directory.  If the file is absent, sensible defaults are used.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::{KernelError, KernelResult};

/// Scale factor mapping INT8 TOPS → energy_per_tick.
/// 1 TOPS ≈ 1 energy unit per tick (adjustable).
const TOPS_SCALE_FACTOR: f64 = 1.0;

/// PIP-001 §4 no-starvation minimum: energy_per_tick must be at least as large as
/// the most expensive basic operation cost so that a single-actor system can
/// always afford to run at least one action per tick.
///
/// Current max basic cost = execute(25) + minimum payload overhead(1) = 26.
const MIN_ENERGY_PER_TICK: i64 = 26;

/// Default INT8 TOPS when no hardware detection or config is provided.
const DEFAULT_INT8_TOPS: f64 = 100.0;

/// Default tick interval in milliseconds.
const DEFAULT_TICK_INTERVAL_MS: u64 = 1000;

/// Stellar configuration — describes the hardware anchor and derived energy rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarConfig {
    /// GPU model string (informational).
    #[serde(default = "default_unknown")]
    pub gpu_model: String,
    /// CPU model string (informational).
    #[serde(default = "default_unknown")]
    pub cpu_model: String,
    /// Hardware INT8 TOPS (tera-operations per second).
    #[serde(default = "default_int8_tops")]
    pub int8_tops: f64,
    /// Computed or overridden energy produced per tick.
    /// If absent in the TOML file, computed from `int8_tops`.
    #[serde(default)]
    pub energy_per_tick: Option<i64>,
    /// Tick interval in milliseconds.
    #[serde(default = "default_tick_interval_ms")]
    pub tick_interval_ms: u64,
    /// Source of the luminosity value: "config" or "detected".
    #[serde(default = "default_source")]
    pub luminosity_source: String,
}

fn default_unknown() -> String {
    "unknown".to_string()
}
fn default_int8_tops() -> f64 {
    DEFAULT_INT8_TOPS
}
fn default_tick_interval_ms() -> u64 {
    DEFAULT_TICK_INTERVAL_MS
}
fn default_source() -> String {
    "config".to_string()
}

impl Default for StellarConfig {
    fn default() -> Self {
        Self {
            gpu_model: default_unknown(),
            cpu_model: default_unknown(),
            int8_tops: DEFAULT_INT8_TOPS,
            energy_per_tick: None,
            tick_interval_ms: DEFAULT_TICK_INTERVAL_MS,
            luminosity_source: default_source(),
        }
    }
}

impl StellarConfig {
    /// Returns the effective energy_per_tick, using the explicit override if
    /// present, otherwise computing from `int8_tops`.
    pub fn effective_energy_per_tick(&self) -> i64 {
        self.energy_per_tick
            .unwrap_or_else(|| compute_energy_per_tick(self.int8_tops))
    }
}

/// Compute energy_per_tick from hardware INT8 TOPS.
///
/// PIP-001 §4 no-starvation constraint: result is always ≥ `MIN_ENERGY_PER_TICK`
/// (currently 26), ensuring at least one execute action can be afforded per tick.
pub fn compute_energy_per_tick(int8_tops: f64) -> i64 {
    let raw = (int8_tops * TOPS_SCALE_FACTOR).round() as i64;
    raw.max(MIN_ENERGY_PER_TICK)
}

/// Load stellar configuration from a TOML file.
///
/// If the file does not exist, returns the default configuration.
/// If the file exists but cannot be parsed, returns an error.
pub fn load_stellar_config(config_path: &Path) -> KernelResult<StellarConfig> {
    if !config_path.exists() {
        return Ok(StellarConfig::default());
    }

    let contents = std::fs::read_to_string(config_path).map_err(|e| {
        KernelError::PolicyViolation(format!(
            "failed to read stellar config at {}: {e}",
            config_path.display()
        ))
    })?;

    let config: StellarConfig = toml::from_str(&contents).map_err(|e| {
        KernelError::PolicyViolation(format!(
            "failed to parse stellar config at {}: {e}",
            config_path.display()
        ))
    })?;

    // Validate: energy_per_tick (if explicitly set) must meet minimum
    if let Some(ept) = config.energy_per_tick
        && ept < MIN_ENERGY_PER_TICK
    {
        return Err(KernelError::PolicyViolation(format!(
            "stellar config: energy_per_tick ({ept}) is below minimum ({MIN_ENERGY_PER_TICK})"
        )));
    }

    // Validate: int8_tops must be positive
    if config.int8_tops <= 0.0 {
        return Err(KernelError::PolicyViolation(format!(
            "stellar config: int8_tops ({}) must be positive",
            config.int8_tops
        )));
    }

    Ok(config)
}

/// Save a default stellar config TOML file (for bootstrapping).
pub fn save_default_stellar_config(config_path: &Path) -> KernelResult<()> {
    let default = StellarConfig::default();
    let toml_str = toml::to_string_pretty(&default).map_err(|e| {
        KernelError::PolicyViolation(format!("failed to serialize default stellar config: {e}"))
    })?;
    std::fs::write(config_path, toml_str).map_err(|e| {
        KernelError::PolicyViolation(format!(
            "failed to write stellar config at {}: {e}",
            config_path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_energy_per_tick_with_default_tops() {
        let ept = compute_energy_per_tick(DEFAULT_INT8_TOPS);
        assert_eq!(ept, 100);
        assert!(ept >= MIN_ENERGY_PER_TICK);
    }

    #[test]
    fn compute_energy_per_tick_enforces_minimum() {
        // Very low TOPS should still produce at least MIN_ENERGY_PER_TICK
        let ept = compute_energy_per_tick(1.0);
        assert_eq!(ept, MIN_ENERGY_PER_TICK);
    }

    #[test]
    fn compute_energy_per_tick_zero_tops() {
        let ept = compute_energy_per_tick(0.0);
        assert_eq!(ept, MIN_ENERGY_PER_TICK);
    }

    #[test]
    fn compute_energy_per_tick_high_tops() {
        let ept = compute_energy_per_tick(500.0);
        assert_eq!(ept, 500);
    }

    #[test]
    fn default_config_effective_energy() {
        let config = StellarConfig::default();
        assert_eq!(config.effective_energy_per_tick(), 100);
    }

    #[test]
    fn config_explicit_override() {
        let config = StellarConfig {
            energy_per_tick: Some(200),
            ..Default::default()
        };
        assert_eq!(config.effective_energy_per_tick(), 200);
    }

    #[test]
    fn load_missing_config_returns_default() {
        let path = std::path::PathBuf::from("/nonexistent/stellar.toml");
        let config = load_stellar_config(&path).unwrap();
        assert_eq!(config.effective_energy_per_tick(), 100);
        assert_eq!(config.luminosity_source, "config");
    }

    #[test]
    fn roundtrip_toml_serialization() {
        let config = StellarConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: StellarConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            config.effective_energy_per_tick(),
            parsed.effective_energy_per_tick()
        );
    }
}
