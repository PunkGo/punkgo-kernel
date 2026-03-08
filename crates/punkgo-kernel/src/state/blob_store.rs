//! Content-addressable blob storage — Git-style filesystem CAS.
//!
//! Event log (SQLite) stores only metadata and OID references.
//! Actual content (text, video, audio, etc.) lives on the filesystem.
//!
//! OID format: `sha256:<hex>` (Git LFS style, self-describing).
//! Path layout: `{root}/sha256/{hex[0:2]}/{hex[2:4]}/{full_hex}` (Docker OCI style).

use std::fs;
use std::path::{Path, PathBuf};

use punkgo_core::errors::{KernelError, KernelResult};
use sha2::{Digest, Sha256};

/// Content-addressable blob store backed by the filesystem.
#[derive(Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Bootstrap the blob store directory structure.
    pub fn bootstrap(&self) -> KernelResult<()> {
        fs::create_dir_all(&self.root)?;
        Ok(())
    }

    /// Store content and return its OID (`sha256:<hex>`).
    /// Idempotent: storing the same content twice returns the same OID.
    pub fn put(&self, data: &[u8]) -> KernelResult<String> {
        let hex = sha256_hex(data);
        let oid = format!("sha256:{hex}");
        let path = self.oid_to_path(&hex);

        if path.exists() {
            return Ok(oid);
        }

        // Write to temp file then rename for atomicity.
        let parent = path
            .parent()
            .ok_or_else(|| KernelError::PolicyViolation("blob path has no parent".to_string()))?;
        fs::create_dir_all(parent)?;

        let tmp_path = parent.join(format!(".tmp-{hex}"));
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, &path)?;

        Ok(oid)
    }

    /// Read content by OID. Returns the raw bytes.
    pub fn get(&self, oid: &str) -> KernelResult<Vec<u8>> {
        let hex = parse_oid(oid)?;
        let path = self.oid_to_path(&hex);

        if !path.exists() {
            return Err(KernelError::PolicyViolation(format!(
                "blob not found: {oid}"
            )));
        }

        let data = fs::read(&path)?;
        Ok(data)
    }

    /// Check if a blob exists.
    pub fn exists(&self, oid: &str) -> bool {
        parse_oid(oid)
            .map(|hex| self.oid_to_path(&hex).exists())
            .unwrap_or(false)
    }

    /// Convert a hex hash to a filesystem path.
    /// Layout: `{root}/sha256/{hex[0:2]}/{hex[2:4]}/{full_hex}`
    fn oid_to_path(&self, hex: &str) -> PathBuf {
        self.root
            .join("sha256")
            .join(&hex[..2])
            .join(&hex[2..4])
            .join(hex)
    }
}

/// Parse a `sha256:<hex>` OID and return the hex portion.
fn parse_oid(oid: &str) -> KernelResult<String> {
    let hex = oid.strip_prefix("sha256:").ok_or_else(|| {
        KernelError::PolicyViolation(format!("invalid OID format (expected sha256:<hex>): {oid}"))
    })?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(KernelError::PolicyViolation(format!(
            "invalid OID hex (expected 64 hex chars): {oid}"
        )));
    }
    Ok(hex.to_string())
}

fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    bytes_to_hex(&hash)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_blob_store() -> (BlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        store.bootstrap().unwrap();
        (store, dir)
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _dir) = temp_blob_store();
        let data = b"hello world";
        let oid = store.put(data).unwrap();
        assert!(oid.starts_with("sha256:"));
        assert_eq!(store.get(&oid).unwrap(), data);
    }

    #[test]
    fn deduplication() {
        let (store, _dir) = temp_blob_store();
        let oid1 = store.put(b"same content").unwrap();
        let oid2 = store.put(b"same content").unwrap();
        assert_eq!(oid1, oid2);
    }

    #[test]
    fn different_content_different_oid() {
        let (store, _dir) = temp_blob_store();
        let oid1 = store.put(b"content A").unwrap();
        let oid2 = store.put(b"content B").unwrap();
        assert_ne!(oid1, oid2);
    }

    #[test]
    fn exists_check() {
        let (store, _dir) = temp_blob_store();
        let oid = store.put(b"test").unwrap();
        assert!(store.exists(&oid));
        assert!(
            !store
                .exists("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        );
    }

    #[test]
    fn get_missing_blob_returns_error() {
        let (store, _dir) = temp_blob_store();
        let result =
            store.get("sha256:0000000000000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_oid_format() {
        let (store, _dir) = temp_blob_store();
        assert!(store.get("md5:abc123").is_err());
        assert!(store.get("sha256:tooshort").is_err());
        assert!(!store.exists("bad-oid"));
    }

    #[test]
    fn binary_content() {
        let (store, _dir) = temp_blob_store();
        let data: Vec<u8> = (0u8..=255).collect();
        let oid = store.put(&data).unwrap();
        assert_eq!(store.get(&oid).unwrap(), data);
    }

    #[test]
    fn empty_content() {
        let (store, _dir) = temp_blob_store();
        let oid = store.put(b"").unwrap();
        assert_eq!(store.get(&oid).unwrap(), b"");
    }
}
