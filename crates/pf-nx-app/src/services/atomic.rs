//! Crash-safe file writes.
//!
//! Write-then-rename, never truncate-in-place: `std::fs::write` truncates first, so a
//! kill/power-cut mid-write (a console loses power mid-write too) leaves a half-file,
//! and the loaders' `unwrap_or_default()` would then silently discard every paired host / all
//! settings. A rename on the same filesystem is atomic; readers see the old file or the new one,
//! never a torn one.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Restore a prior destination left in `.bak` if power was lost between two
/// rename operations on a filesystem that refuses rename-over-existing.
/// Never overwrites an existing destination (which may already be newer).
pub fn recover(path: &Path) -> Result<()> {
    let bak = suffix(path, ".bak");
    if !path.exists() && bak.exists() {
        fs::rename(&bak, path)
            .with_context(|| format!("recover {} from backup", path.display()))?;
    }
    Ok(())
}

fn suffix(path: &Path, s: &str) -> PathBuf {
    let mut out = path.as_os_str().to_os_string();
    out.push(s);
    PathBuf::from(out)
}

/// Durable: the staged file is synced before the rename. A rename is atomic against a kill, but
/// against a power cut only once the data it points at has reached the disk — otherwise the
/// filesystem may commit the rename first and leave an empty file under the real name. For the
/// files that cannot be refetched: the settings document and the client identity.
pub fn write(path: &Path, contents: &str, what: &str) -> Result<()> {
    stage_and_rename(path, &[contents.as_bytes()], what, true)
}

/// Same discipline for byte payloads that arrive in pieces (a header plus a pixel buffer, say):
/// the parts are written in order, so nothing has to be concatenated into one allocation first.
///
/// Not synced: this is for caches, where a write lost to a power cut costs one refetch, and a
/// sync per entry would cost every fetch.
pub fn write_parts(path: &Path, parts: &[&[u8]], what: &str) -> Result<()> {
    stage_and_rename(path, parts, what, false)
}

/// `.tmp` is appended to the whole filename rather than replacing an extension, so two files
/// that differ only by extension can never stage to the same path.
fn stage_and_rename(path: &Path, parts: &[&[u8]], what: &str, durable: bool) -> Result<()> {
    recover(path)?;
    let tmp = suffix(path, ".tmp");
    let mut file = File::create(&tmp).with_context(|| format!("create {what} (tmp)"))?;
    for part in parts {
        file.write_all(part).with_context(|| format!("write {what} (tmp)"))?;
    }
    if durable {
        file.sync_all().with_context(|| format!("sync {what} (tmp)"))?;
    }
    drop(file);
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || e.raw_os_error() == Some(17) => {
            // Switch SD mounts may refuse rename-over-existing. Preserve the
            // prior file under .bak until the new name is safely installed.
            // If a crash occurs between the moves, recover() restores .bak.
            let bak = suffix(path, ".bak");
            if bak.exists() {
                fs::remove_file(&bak)
                    .with_context(|| format!("clear old {what} backup"))?;
            }
            fs::rename(path, &bak)
                .with_context(|| format!("back up previous {what}"))?;
            if let Err(err) = fs::rename(&tmp, path) {
                // Do not hide the original install error. Restoration is
                // best-effort here, and recover() runs on the next boot.
                let _ = fs::rename(&bak, path);
                return Err(err).with_context(|| format!("install staged {what}"));
            }
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("rename {what} into place")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovers_missing_destination_from_backup_without_overwriting_existing() {
        let dir = std::env::temp_dir().join(format!("pf2-atomic-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let backup = suffix(&path, ".bak");
        fs::write(&backup, b"previous").unwrap();
        recover(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        fs::write(&backup, b"older").unwrap();
        recover(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        fs::remove_dir_all(&dir).unwrap();
    }
}
