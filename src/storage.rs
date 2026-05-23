//! Content-addressed blob store on the local filesystem.
//!
//! Files are written to `<root>/blobs/<aa>/<bb>/<sha256>` (2-level
//! fan-out so a single dir never holds millions of inodes). The sha256
//! is computed during the write so callers don't have to re-hash.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(root.join("blobs"))
            .with_context(|| format!("creating storage root {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path on disk for a blob with the given sha256 hex string. Pure
    /// (no I/O) — useful for tests and for handing the path to
    /// `serve_file`-style middleware.
    pub fn path_for(&self, sha256_hex: &str) -> PathBuf {
        let (aa, bb) = split_prefix(sha256_hex);
        self.root.join("blobs").join(aa).join(bb).join(sha256_hex)
    }

    /// Hash + persist `bytes`. Returns `(sha256_hex, relative_path)`.
    /// Idempotent — if a blob with the same sha already exists the
    /// existing file is kept and the write is skipped (cheap dedup).
    pub async fn put(&self, bytes: &[u8]) -> Result<StoredBlob> {
        let sha = sha256_hex(bytes);
        let dest = self.path_for(&sha);
        let rel = dest
            .strip_prefix(&self.root)
            .map_err(|_| anyhow!("dest not under storage root"))?
            .to_path_buf();

        if dest.exists() {
            return Ok(StoredBlob {
                sha256: sha,
                path: rel,
                size: bytes.len() as u64,
            });
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut f = fs::File::create(&dest)
            .await
            .with_context(|| format!("creating blob {}", dest.display()))?;
        f.write_all(bytes).await?;
        f.flush().await?;

        Ok(StoredBlob {
            sha256: sha,
            path: rel,
            size: bytes.len() as u64,
        })
    }

    /// Read the blob bytes for a previously-stored sha256. Returns
    /// `Ok(None)` when the file is gone (cleanup race, manual delete).
    pub async fn get(&self, sha256_hex: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_for(sha256_hex);
        match fs::read(&path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredBlob {
    pub sha256: String,
    pub path: PathBuf,
    pub size: u64,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn split_prefix(sha: &str) -> (&str, &str) {
    // sha256 is always at least 4 hex chars; defensive but never trips
    // in practice since we feed our own hashes.
    if sha.len() < 4 {
        return ("xx", "xx");
    }
    (&sha[0..2], &sha[2..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
        let blob = store.put(b"hello world").await.unwrap();
        assert_eq!(
            blob.sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let got = store.get(&blob.sha256).await.unwrap().unwrap();
        assert_eq!(got, b"hello world");
    }

    #[tokio::test]
    async fn put_is_idempotent_for_same_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
        let a = store.put(b"x").await.unwrap();
        let b = store.put(b"x").await.unwrap();
        assert_eq!(a.sha256, b.sha256);
        assert_eq!(a.path, b.path);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
        let none = store.get("deadbeef").await.unwrap();
        assert!(none.is_none());
    }
}
