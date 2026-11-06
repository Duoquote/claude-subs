//! Small file helpers: JSON reads that tell "missing" from "broken", atomic replacement, stamps.

use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The JSON at `path`, `None` when the file does not exist. A file that does not parse is an error
/// (never treated as empty: that is how a config gets wiped).
pub fn read_json(path: &Path) -> Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(value))
}

/// Replaces `path` with `bytes` in one step: a sibling temp file, then a rename over the target
/// (MoveFileEx with replace). A reader sees the old file or the new one, never half of either. The
/// rename is retried briefly: on Windows a reader holding the file without delete-sharing makes it
/// fail for a moment.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("target has no parent directory")?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().context("target has no file name")?.to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.subs-{}-{:08x}.tmp",
        std::process::id(),
        rand::random::<u32>()
    ));
    write_private(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    let mut last = None;
    for attempt in 0..20 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(25 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    match last {
        Some(e) => Err(e).with_context(|| format!("replacing {}", path.display())),
        None => bail!("replacing {} failed", path.display()),
    }
}

/// A new file readable by this user alone where the platform has modes (0600): what we write holds
/// tokens, or sits beside them (`.credentials.json` is 0600 on Linux).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// A change stamp (modified time in ns, size); `None` when the file is missing.
pub fn stamp(path: &Path) -> Option<(u128, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Some((modified, meta.len()))
}

/// Age of `path`'s modified time, `None` when it is missing.
pub fn age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(modified).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_atomic(&path, b"{\"a\":1}").unwrap();
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(read_json(&path).unwrap().unwrap()["a"], 2);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "only the target remains: {names:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "readable by this user alone");
        }
    }

    #[test]
    fn missing_is_none_and_broken_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_json(&dir.path().join("nope.json")).unwrap().is_none());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(read_json(&bad).is_err());
    }
}
