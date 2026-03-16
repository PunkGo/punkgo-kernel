//! Ed25519 signing for checkpoint authentication.
//!
//! On first boot, a keypair is generated and stored at `{state_dir}/signing_key`.
//! Every checkpoint is signed. The public key is embedded in the checkpoint
//! extension line for third-party verification.
//!
//! Format: `sig/ed25519:<pubkey_hex>:<signature_hex>\n`
//!
//! This module does NOT provide PKI, key rotation, or witness cosigning.
//! Those are Phase 2+ concerns. What it provides:
//! - Identity binding (this checkpoint came from this kernel instance)
//! - Format readiness (checkpoint structure is ready for TSA/witness upgrades)

use std::path::Path;

use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey as DalekSigningKey, Verifier, VerifyingKey};

/// Wrapper around Ed25519 keypair for checkpoint signing.
#[derive(Clone)]
pub struct SigningKey {
    inner: DalekSigningKey,
}

impl SigningKey {
    /// Generate a new random keypair and save the 32-byte seed to disk.
    pub fn generate_and_save(path: &Path) -> Result<Self> {
        let mut rng = rand::thread_rng();
        let inner = DalekSigningKey::generate(&mut rng);
        std::fs::write(path, inner.to_bytes())
            .with_context(|| format!("failed to write signing key to {}", path.display()))?;
        Ok(Self { inner })
    }

    /// Load an existing keypair from the 32-byte seed on disk.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read signing key from {}", path.display()))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("signing key must be exactly 32 bytes"))?;
        let inner = DalekSigningKey::from_bytes(&bytes);
        Ok(Self { inner })
    }

    /// Load from disk if exists, otherwise generate and save.
    pub fn load_or_generate(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Self::generate_and_save(path)
        }
    }

    /// Sign a message. Returns the 64-byte signature.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.inner.sign(msg).to_bytes()
    }

    /// Verify a signature against this key's public key.
    pub fn verify(&self, msg: &[u8], sig_bytes: &[u8; 64]) -> bool {
        let Ok(sig) = Signature::from_slice(sig_bytes) else {
            return false;
        };
        self.inner.verifying_key().verify(msg, &sig).is_ok()
    }

    /// Public key as 32-byte array.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.inner.verifying_key().to_bytes()
    }

    /// Public key as hex string (64 chars).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_bytes())
    }

    /// Build the checkpoint extension line: `sig/ed25519:<pubkey>:<sig>\n`
    pub fn sign_checkpoint(&self, checkpoint_body: &[u8]) -> String {
        let sig = self.sign(checkpoint_body);
        format!(
            "sig/ed25519:{}:{}\n",
            self.public_key_hex(),
            hex::encode(sig)
        )
    }
}

/// Verify a checkpoint signature given raw components.
/// Used by jack for offline verification without the private key.
pub fn verify_checkpoint_signature(pubkey_hex: &str, msg: &[u8], sig_hex: &str) -> bool {
    let Ok(pk_bytes) = hex::decode(pubkey_hex) else {
        return false;
    };
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.try_into() else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&sig_bytes) else {
        return false;
    };
    vk.verify(msg, &sig).is_ok()
}

/// Parse a `sig/ed25519:<pubkey>:<sig>` extension line.
/// Returns (pubkey_hex, sig_hex) if valid.
pub fn parse_sig_extension(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("sig/ed25519:")?;
    let rest = rest.strip_suffix('\n').unwrap_or(rest);
    let (pubkey, sig) = rest.split_once(':')?;
    if pubkey.len() == 64 && sig.len() == 128 {
        Some((pubkey, sig))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generate_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("signing_key");
        let sk1 = SigningKey::generate_and_save(&path).unwrap();
        let sk2 = SigningKey::load(&path).unwrap();
        assert_eq!(sk1.public_key_bytes(), sk2.public_key_bytes());
    }

    #[test]
    fn sign_and_verify() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("signing_key");
        let sk = SigningKey::generate_and_save(&path).unwrap();
        let msg = b"punkgo/kernel\n42\nhash=\n";
        let sig = sk.sign(msg);
        assert!(sk.verify(msg, &sig));
    }

    #[test]
    fn wrong_message_fails_verify() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("signing_key");
        let sk = SigningKey::generate_and_save(&path).unwrap();
        let sig = sk.sign(b"correct");
        assert!(!sk.verify(b"wrong", &sig));
    }

    #[test]
    fn load_or_generate_creates_on_first_call() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("signing_key");
        assert!(!path.exists());
        let sk = SigningKey::load_or_generate(&path).unwrap();
        assert!(path.exists());
        assert_eq!(sk.public_key_hex().len(), 64);
    }

    #[test]
    fn sign_checkpoint_produces_valid_extension() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("signing_key");
        let sk = SigningKey::generate_and_save(&path).unwrap();

        let body = b"punkgo/kernel\n100\naBcDeFgH=\n";
        let ext = sk.sign_checkpoint(body);

        assert!(ext.starts_with("sig/ed25519:"));
        assert!(ext.ends_with('\n'));

        // Parse and verify
        let (pubkey, sig) = parse_sig_extension(&ext).unwrap();
        assert!(verify_checkpoint_signature(pubkey, body, sig));
    }

    #[test]
    fn parse_sig_extension_rejects_garbage() {
        assert!(parse_sig_extension("garbage").is_none());
        assert!(parse_sig_extension("sig/ed25519:short:short").is_none());
        assert!(parse_sig_extension("sig/rsa:abc:def").is_none());
    }

    #[test]
    fn verify_checkpoint_signature_rejects_wrong_key() {
        let dir = tempdir().unwrap();
        let sk1 = SigningKey::generate_and_save(&dir.path().join("k1")).unwrap();
        let sk2 = SigningKey::generate_and_save(&dir.path().join("k2")).unwrap();

        let msg = b"test message";
        let sig = sk1.sign(msg);
        // Verify with sk2's public key should fail
        assert!(!verify_checkpoint_signature(
            &sk2.public_key_hex(),
            msg,
            &hex::encode(sig)
        ));
    }
}
