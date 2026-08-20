use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("failed to create asset cache directory {path}: {source}")]
    CreateCache {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to read cached asset {path}: {source}")]
    ReadCache {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to write cached asset {path}: {source}")]
    WriteCache {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to fetch {url}: {message}")]
    Fetch { url: String, message: String },

    #[error("unsupported asset source {src}")]
    UnsupportedSource { src: String },
}

#[derive(Debug, Clone)]
pub struct ResolvedAsset {
    pub src: String,
    pub cache_path: PathBuf,
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AssetCache {
    root: PathBuf,
    max_bytes: u64,
    package_assets: BTreeMap<String, Vec<u8>>,
}

impl AssetCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_bytes: 250 * 1024 * 1024,
            package_assets: BTreeMap::new(),
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn with_package_assets(mut self, assets: BTreeMap<String, Vec<u8>>) -> Self {
        self.package_assets = assets;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve(&self, src: &str) -> Result<ResolvedAsset, AssetError> {
        if let Some(bytes) = self.package_assets.get(src) {
            return Ok(ResolvedAsset {
                src: src.to_owned(),
                cache_path: PathBuf::from(src),
                media_type: media_type_for(src),
                bytes: bytes.clone(),
            });
        }

        if is_remote_url(src) {
            return self.resolve_remote(src);
        }

        let path = PathBuf::from(src);
        if path.exists() {
            let bytes = fs::read(&path).map_err(|source| AssetError::ReadCache {
                path: path.clone(),
                source,
            })?;
            return Ok(ResolvedAsset {
                src: src.to_owned(),
                cache_path: path,
                media_type: media_type_for(src),
                bytes,
            });
        }

        Err(AssetError::UnsupportedSource {
            src: src.to_owned(),
        })
    }

    fn resolve_remote(&self, url: &str) -> Result<ResolvedAsset, AssetError> {
        fs::create_dir_all(&self.root).map_err(|source| AssetError::CreateCache {
            path: self.root.clone(),
            source,
        })?;

        let cache_path = self.cache_path_for(url);
        if cache_path.exists() {
            let bytes = fs::read(&cache_path).map_err(|source| AssetError::ReadCache {
                path: cache_path.clone(),
                source,
            })?;
            return Ok(ResolvedAsset {
                src: url.to_owned(),
                cache_path,
                media_type: media_type_for(url),
                bytes,
            });
        }

        let bytes = fetch_url(url)?;
        fs::write(&cache_path, &bytes).map_err(|source| AssetError::WriteCache {
            path: cache_path.clone(),
            source,
        })?;
        self.evict_oldest()?;

        Ok(ResolvedAsset {
            src: url.to_owned(),
            cache_path,
            bytes,
            media_type: media_type_for(url),
        })
    }

    fn cache_path_for(&self, url: &str) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(url.as_bytes());
        let digest = hash.finalize();
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let extension = extension_for(url).unwrap_or("asset");
        self.root.join(format!("{hex}.{extension}"))
    }

    fn evict_oldest(&self) -> Result<(), AssetError> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };

        let mut files = Vec::new();
        let mut total = 0_u64;
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let len = metadata.len();
            total += len;
            files.push((entry.path(), len, metadata.modified().unwrap_or(UNIX_EPOCH)));
        }

        if total <= self.max_bytes {
            return Ok(());
        }

        files.sort_by_key(|(_, _, modified)| *modified);
        for (path, len, _) in files {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }

        Ok(())
    }
}

fn is_remote_url(src: &str) -> bool {
    src.starts_with("https://") || src.starts_with("http://")
}

#[cfg(feature = "net")]
fn fetch_url(url: &str) -> Result<Vec<u8>, AssetError> {
    let response = ureq::get(url).call().map_err(|err| AssetError::Fetch {
        url: url.to_owned(),
        message: err.to_string(),
    })?;

    let mut body = response.into_body();
    body.read_to_vec().map_err(|err| AssetError::Fetch {
        url: url.to_owned(),
        message: err.to_string(),
    })
}

#[cfg(not(feature = "net"))]
fn fetch_url(url: &str) -> Result<Vec<u8>, AssetError> {
    Err(AssetError::Fetch {
        url: url.to_owned(),
        message: "network support is disabled; build with the \"net\" feature or                   supply this asset through the .gui package"
            .to_owned(),
    })
}

fn extension_for(src: &str) -> Option<&'static str> {
    let path = src.split('?').next().unwrap_or(src);
    let extension = Path::new(path).extension()?.to_str()?;
    match extension {
        "svg" => Some("svg"),
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        "ttf" => Some("ttf"),
        "otf" => Some("otf"),
        "woff" => Some("woff"),
        "woff2" => Some("woff2"),
        _ => None,
    }
}

fn media_type_for(src: &str) -> Option<String> {
    match extension_for(src)? {
        "svg" => Some("image/svg+xml".to_owned()),
        "png" => Some("image/png".to_owned()),
        "jpg" => Some("image/jpeg".to_owned()),
        "webp" => Some("image/webp".to_owned()),
        "ttf" => Some("font/ttf".to_owned()),
        "otf" => Some("font/otf".to_owned()),
        "woff" => Some("font/woff".to_owned()),
        "woff2" => Some("font/woff2".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_stable_and_keeps_extension() {
        let cache = AssetCache::new(std::env::temp_dir().join("dotgui-renderer-cache-test"));
        let first = cache.cache_path_for("https://example.com/icon.svg?color=%23fff");
        let second = cache.cache_path_for("https://example.com/icon.svg?color=%23fff");

        assert_eq!(first, second);
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("svg"));
    }

    #[test]
    fn resolves_packaged_assets_before_other_sources() {
        let mut assets = BTreeMap::new();
        assets.insert("assets/icon.svg".to_owned(), b"<svg />".to_vec());
        let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(assets);

        let resolved = cache.resolve("assets/icon.svg").expect("asset resolves");

        assert_eq!(resolved.bytes, b"<svg />");
        assert_eq!(resolved.media_type.as_deref(), Some("image/svg+xml"));
    }

    #[test]
    fn missing_packaged_assets_are_not_resolved_as_remote_icons() {
        let cache = AssetCache::new(std::env::temp_dir());

        assert!(matches!(
            cache.resolve("assets/missing.svg"),
            Err(AssetError::UnsupportedSource { .. })
        ));
    }
}
