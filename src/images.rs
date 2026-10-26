//! Images in issues: fetching, caching and decoding.
//!
//! Only files on uploads.linear.app are fetched. `issue show --signed-urls`
//! makes their URLs open without the API key for a while; once one has
//! expired (401/403), `linearctl file download` fetches it with the key.
//! The bytes behind an upload path never change, so the disk cache is keyed
//! by the path alone (the signature is dropped) and never revalidated.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};

use crate::ctl::Runner;

/// The only host images are fetched from.
pub const UPLOADS: &str = "https://uploads.linear.app/";
/// The largest file fetched.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// The largest image decoded, per side.
pub const MAX_SIDE: u32 = 8192;
/// Decoded images are shrunk to fit this box: more than any pane shows.
pub const SHOW_W: u32 = 1600;
pub const SHOW_H: u32 = 1200;

/// The path of an upload URL (`/org/uuid/uuid`), without its signature;
/// `None` for any other URL.
pub fn upload_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(UPLOADS)?;
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    (!path.is_empty() && !path.contains("..")).then(|| &url[UPLOADS.len() - 1..UPLOADS.len() + path.len()])
}

/// The cache key for an upload URL: SHA-256 of its path, in hex.
pub fn cache_key(url: &str) -> Option<String> {
    let path = upload_path(url)?;
    Some(
        Sha256::digest(path.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// 401 or 403: the signature expired (or there never was one).
    Expired,
    Status(u16),
    TooLarge,
    Network(String),
}

/// Where image bytes come from.
pub trait Source: Send + Sync {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}

/// HTTPS with no redirects, a deadline and a size cap.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Self {
        Self::with_https_only(true)
    }

    /// `https_only: false` is for tests against a local server.
    pub fn with_https_only(https_only: bool) -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(https_only)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

impl Source for Http {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let mut resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {}
            401 | 403 => return Err(FetchError::Expired),
            s => return Err(FetchError::Status(s)),
        }
        let mut body = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|e| FetchError::Network(e.to_string()))?;
        if body.len() as u64 > MAX_BYTES {
            return Err(FetchError::TooLarge);
        }
        Ok(body)
    }
}

/// The demo workspace's uploads, built in: `uploads.linear.app/demo/NAME`.
pub struct Demo;

impl Source for Demo {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        match upload_path(url) {
            Some("/demo/checkout-error.png") => {
                Ok(include_bytes!("../demo/images/checkout-error.png").to_vec())
            }
            _ => Err(FetchError::Status(404)),
        }
    }
}

/// Image files on disk, least recently used first out.
pub struct DiskCache {
    pub dir: PathBuf,
    pub budget: u64,
}

impl DiskCache {
    pub fn new(dir: PathBuf, budget: u64) -> Self {
        Self { dir, budget }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let p = self.path(key);
        let bytes = fs::read(&p).ok()?;
        // Touch it, so eviction sees it as recently used.
        let _ = fs::File::options()
            .append(true)
            .open(&p)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        Some(bytes)
    }

    /// Store, then evict the oldest files over the budget. Errors are
    /// ignored: the cache is only an optimisation.
    pub fn put(&self, key: &str, bytes: &[u8]) {
        if create_private_dir(&self.dir).is_err() {
            return;
        }
        let tmp = self.dir.join(format!(".{key}.{}", std::process::id()));
        if write_private(&tmp, bytes).is_ok() {
            let _ = fs::rename(&tmp, self.path(key));
        }
        self.evict();
    }

    pub fn evict(&self) {
        let Ok(rd) = fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
            .filter_map(Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                if m.is_file() {
                    Some((m.modified().ok()?, m.len(), e.path()))
                } else {
                    None
                }
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, path) in files {
            if total <= self.budget {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = fs::File::options();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    std::io::Write::write_all(&mut opts.open(path)?, bytes)
}

/// Decode PNG, JPEG, GIF (first frame), WebP or BMP, within limits, and
/// shrink it to what a pane can show.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let format = image::guess_format(bytes).map_err(|_| "not an image".to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Bmp
    ) {
        return Err(format!("unsupported image format {format:?}"));
    }
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    Ok(if img.width() > SHOW_W || img.height() > SHOW_H {
        img.resize(SHOW_W, SHOW_H, image::imageops::FilterType::Triangle)
    } else {
        img
    })
}

/// Fetches through the cache, falling back to linearctl when a signature
/// has expired.
pub struct Loader {
    pub source: Box<dyn Source>,
    pub cache: Option<DiskCache>,
    /// linearctl, for `file download` with the key.
    pub fallback: Option<Runner>,
}

impl Loader {
    pub fn load(&self, url: &str) -> Result<DynamicImage, String> {
        let key = cache_key(url).ok_or("not a Linear upload")?;
        if let Some(bytes) = self.cache.as_ref().and_then(|c| c.get(&key))
            && let Ok(img) = decode(&bytes)
        {
            return Ok(img);
        }
        let bytes = match self.source.fetch(url) {
            Ok(b) => b,
            Err(FetchError::Expired) => self.download(url, &key)?,
            Err(FetchError::Status(s)) => return Err(format!("HTTP {s}")),
            Err(FetchError::TooLarge) => return Err(format!("larger than {} MB", MAX_BYTES / 1024 / 1024)),
            Err(FetchError::Network(e)) => return Err(e),
        };
        let img = decode(&bytes)?;
        if let Some(c) = &self.cache {
            c.put(&key, &bytes);
        }
        Ok(img)
    }

    /// `linearctl file download` into a private staging directory.
    fn download(&self, url: &str, key: &str) -> Result<Vec<u8>, String> {
        let runner = self.fallback.as_ref().ok_or("the image link expired")?;
        let stage = std::env::temp_dir().join(format!("linear-tui-{}", std::process::id()));
        create_private_dir(&stage).map_err(|e| e.to_string())?;
        let mut runner = runner.clone();
        runner.cwd = Some(stage.clone());
        let path = upload_path(url).expect("checked by cache_key");
        let unsigned = format!("{}{}", &UPLOADS[..UPLOADS.len() - 1], path);
        let argv: Vec<String> = ["file", "download", &unsigned, "--output", key, "--yes", "--json"]
            .map(String::from)
            .to_vec();
        let result = runner.run(&argv).map_err(|f| f.message());
        let file = stage.join(key);
        let bytes = result.and_then(|_| fs::read(&file).map_err(|e| e.to_string()));
        let _ = fs::remove_file(&file);
        bytes
    }
}

/// The default cache directory: `$XDG_CACHE_HOME/linear-tui/<org>/images`,
/// else `~/Library/Caches/...` on macOS or `~/.cache/...`.
pub fn cache_dir(org: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME")?);
            Some(if cfg!(target_os = "macos") {
                home.join("Library/Caches")
            } else {
                home.join(".cache")
            })
        })?;
    let org: String = org
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    Some(
        base.join("linear-tui")
            .join(if org.is_empty() { "default".into() } else { org })
            .join("images"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const DEMO_URL: &str = "https://uploads.linear.app/demo/checkout-error.png?signature=demo";

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("linear-tui-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn upload_paths() {
        assert_eq!(
            upload_path("https://uploads.linear.app/a/b/c?signature=x"),
            Some("/a/b/c")
        );
        assert_eq!(upload_path("https://uploads.linear.app/a/b#frag"), Some("/a/b"));
        assert_eq!(upload_path("https://uploads.linear.app/"), None);
        assert_eq!(upload_path("https://uploads.linear.app/../x"), None);
        assert_eq!(upload_path("http://uploads.linear.app/a"), None);
        assert_eq!(upload_path("https://uploads.linear.app.evil.example/a"), None);
        assert_eq!(upload_path("https://example.com/a.png"), None);
    }

    #[test]
    fn cache_keys_ignore_the_signature() {
        let a = cache_key("https://uploads.linear.app/o/f?signature=1").unwrap();
        assert_eq!(
            a,
            cache_key("https://uploads.linear.app/o/f?signature=2").unwrap()
        );
        assert_ne!(a, cache_key("https://uploads.linear.app/o/g").unwrap());
        assert_eq!(a.len(), 64);
        assert_eq!(cache_key("https://example.com/x"), None);
    }

    #[test]
    fn decodes_the_demo_image() {
        let img = decode(&Demo.fetch(DEMO_URL).unwrap()).unwrap();
        assert_eq!((img.width(), img.height()), (588, 376));
        assert_eq!(
            Demo.fetch("https://uploads.linear.app/demo/nope.png"),
            Err(FetchError::Status(404))
        );
    }

    #[test]
    fn rejects_what_is_not_a_supported_image() {
        assert_eq!(decode(b"hello").unwrap_err(), "not an image");
        // A TIFF header: a real image format, but not one we decode.
        assert!(
            decode(b"II*\0\x08\0\0\0")
                .unwrap_err()
                .starts_with("unsupported image format")
        );
        // A PNG signature with nothing after it.
        assert!(decode(b"\x89PNG\r\n\x1a\n").is_err());
    }

    #[test]
    fn shrinks_big_images() {
        let big = DynamicImage::new_rgb8(3200, 1000);
        let mut png = Vec::new();
        big.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let img = decode(&png).unwrap();
        assert_eq!((img.width(), img.height()), (1600, 500));
    }

    #[test]
    fn disk_cache_round_trip_and_eviction() {
        let dir = tmp("cache");
        let c = DiskCache::new(dir.clone(), 10);
        assert_eq!(c.get("a"), None);
        c.put("a", b"12345");
        assert_eq!(c.get("a").as_deref(), Some(&b"12345"[..]));
        std::thread::sleep(Duration::from_millis(20));
        c.put("b", b"67890");
        std::thread::sleep(Duration::from_millis(20));
        c.get("a"); // a is now the most recently used
        std::thread::sleep(Duration::from_millis(20));
        c.put("c", b"xyz");
        assert!(c.get("b").is_none(), "least recently used goes first");
        assert!(c.get("a").is_some() && c.get("c").is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(
                fs::metadata(dir.join("a")).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(&dir).unwrap();
        // A cache that cannot be created is silently skipped.
        let blocked = tmp("blocked");
        fs::write(&blocked, b"a file, not a dir").unwrap();
        DiskCache::new(blocked.join("sub"), 10).put("k", b"v");
        DiskCache::new(blocked.join("sub"), 10).evict();
        fs::remove_file(&blocked).unwrap();
    }

    /// Answers from a script, counting calls.
    struct Scripted(Mutex<Vec<Result<Vec<u8>, FetchError>>>);

    impl Source for Scripted {
        fn fetch(&self, _: &str) -> Result<Vec<u8>, FetchError> {
            self.0.lock().unwrap().remove(0)
        }
    }

    fn loader(answers: Vec<Result<Vec<u8>, FetchError>>, cache: Option<DiskCache>) -> Loader {
        Loader {
            source: Box::new(Scripted(Mutex::new(answers))),
            cache,
            fallback: None,
        }
    }

    #[test]
    fn loader_uses_the_cache_after_the_first_fetch() {
        let png = Demo.fetch(DEMO_URL).unwrap();
        let dir = tmp("loader");
        let l = loader(vec![Ok(png)], Some(DiskCache::new(dir.clone(), 1 << 20)));
        assert!(l.load(DEMO_URL).is_ok());
        // The script is empty now: a second fetch would panic.
        assert!(
            l.load("https://uploads.linear.app/demo/checkout-error.png?signature=other")
                .is_ok()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_cache_entry_is_refetched() {
        let dir = tmp("corrupt");
        let cache = DiskCache::new(dir.clone(), 1 << 20);
        cache.put(&cache_key(DEMO_URL).unwrap(), b"garbage");
        let l = loader(vec![Ok(Demo.fetch(DEMO_URL).unwrap())], Some(cache));
        assert!(l.load(DEMO_URL).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loader_errors() {
        assert_eq!(
            loader(vec![], None)
                .load("https://example.com/x.png")
                .unwrap_err(),
            "not a Linear upload"
        );
        assert_eq!(
            loader(vec![Err(FetchError::Status(500))], None)
                .load(DEMO_URL)
                .unwrap_err(),
            "HTTP 500"
        );
        assert_eq!(
            loader(vec![Err(FetchError::TooLarge)], None)
                .load(DEMO_URL)
                .unwrap_err(),
            "larger than 20 MB"
        );
        assert_eq!(
            loader(vec![Err(FetchError::Network("reset".into()))], None)
                .load(DEMO_URL)
                .unwrap_err(),
            "reset"
        );
        assert_eq!(
            loader(vec![Err(FetchError::Expired)], None)
                .load(DEMO_URL)
                .unwrap_err(),
            "the image link expired"
        );
        assert_eq!(
            loader(vec![Ok(b"text".to_vec())], None)
                .load(DEMO_URL)
                .unwrap_err(),
            "not an image"
        );
    }

    #[test]
    fn cache_dirs() {
        let d = cache_dir("north wind/../x").unwrap();
        assert!(d.ends_with("linear-tui/northwindx/images"), "{d:?}");
        assert!(cache_dir("").unwrap().ends_with("linear-tui/default/images"));
    }
}
