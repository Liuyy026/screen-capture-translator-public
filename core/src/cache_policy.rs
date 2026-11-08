//! Eviction is restricted to reproducible SR/display assets. OCR, translations,
//! edits, source images and model weights are deliberately outside these roots.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    fs::{self, File, FileTimes, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}},
    time::{Duration, SystemTime},
};
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

pub const LIMIT_BYTES: u64 = 5_000_000_000;
const LOW_WATER_BYTES: u64 = 4_500_000_000;
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub removed_entries: usize,
    pub expired_entries: usize,
    pub protected_entries: usize,
    pub failed_entries: usize,
    pub over_limit: bool,
}

pub struct Manager {
    sr: PathBuf,
    display: PathBuf,
    gate: Arc<RwLock<()>>,
    viewed: Mutex<(u64, Option<File>)>,
    serial: AtomicU64,
    last: Mutex<Option<Report>>,
}

// Keep the filesystem locks alive through every image read/write/await. They
// also coordinate independent candidate processes sharing the same cache.
pub struct Activity {
    _gate: OwnedRwLockReadGuard<()>,
    _files: Vec<File>,
}

fn open_lock(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?)
}

fn try_lock(file: &File, exclusive: bool) -> Result<bool> {
    let mode = if exclusive { libc::LOCK_EX } else { libc::LOCK_SH };
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0 { return Ok(true); }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock { return Ok(false); }
    Err(error.into())
}

fn plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}
fn hex_key(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn touch(path: &Path) -> Result<()> {
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?
        .set_times(FileTimes::new().set_modified(SystemTime::now()))?;
    Ok(())
}
pub fn touch_sr(directory: &Path) -> Result<()> {
    open_lock(&directory.join(".last-used"))?
        .set_times(FileTimes::new().set_modified(SystemTime::now()))?;
    Ok(())
}

struct Entry {
    path: PathBuf,
    bytes: u64,
    last_used: SystemTime,
    directory: bool,
}

impl Manager {
    pub fn new(sr: &Path, display: &Path) -> Result<Arc<Self>> {
        fs::create_dir_all(sr)?;
        fs::create_dir_all(display)?;
        ensure!(plain_dir(sr) && plain_dir(display), "缓存目录不能是符号链接");
        let sr = sr.canonicalize()?;
        let display = display.canonicalize()?;
        ensure!(!sr.starts_with(&display) && !display.starts_with(&sr), "缓存目录不能重叠");
        Ok(Arc::new(Self { sr, display, gate: Arc::new(RwLock::new(())),
            viewed: Mutex::new((0, None)), serial: AtomicU64::new(0), last: Mutex::new(None) }))
    }

    fn locks(&self, exclusive: bool) -> Result<Option<Vec<File>>> {
        let mut files = Vec::new();
        for root in [&self.sr, &self.display] {
            ensure!(plain_dir(root), "缓存目录不可用");
            let file = open_lock(&root.join(".cache-maintenance.lock"))?;
            if !try_lock(&file, exclusive)? { return Ok(None); }
            files.push(file);
        }
        Ok(Some(files))
    }

    pub async fn activity(&self) -> Result<Activity> {
        let gate = self.gate.clone().read_owned().await;
        loop {
            if let Some(files) = self.locks(false)? { return Ok(Activity { _gate: gate, _files: files }); }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn view_ticket(&self) -> u64 { self.serial.fetch_add(1, Ordering::Relaxed) + 1 }

    // Called inside Activity, before returning an effective-image path to Swift.
    // An entry lock outlives the request, protecting the visible image even in
    // another process's eviction pass. Older, slow requests cannot replace it.
    pub fn pin_view(&self, ticket: u64, image: &Path) -> Result<()> {
        let mut viewed = self.viewed.lock().unwrap();
        if ticket < viewed.0 { return Ok(()); }
        let pin = if let Some(dir) = image.parent().filter(|dir|
            dir.parent() == Some(self.sr.as_path()) && image.file_name().is_some_and(|n| n == "effective.png")
            && dir.file_name().and_then(|n| n.to_str()).is_some_and(hex_key)) {
            ensure!(plain_dir(dir), "增强缓存目录不可用");
            let file = open_lock(&dir.join(".in-use"))?;
            ensure!(try_lock(&file, false)?, "增强缓存正在清理，请重试");
            Some(file)
        } else { None };
        *viewed = (ticket, pin);
        Ok(())
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"limit_bytes": LIMIT_BYTES, "low_water_bytes": LOW_WATER_BYTES,
            "unused_days": 30, "scope": "reproducible_sr_and_display_only", "last_sweep": *self.last.lock().unwrap()})
    }

    pub async fn maintain(self: &Arc<Self>) -> Result<Option<Report>> {
        let Ok(gate) = self.gate.clone().try_write_owned() else { return Ok(None); };
        let Some(files) = self.locks(true)? else { return Ok(None); };
        let manager = self.clone();
        let report = tokio::task::spawn_blocking(move || {
            let (_gate, _files) = (gate, files);
            manager.sweep(SystemTime::now(), LIMIT_BYTES, LOW_WATER_BYTES)
        }).await??;
        *self.last.lock().unwrap() = Some(report.clone());
        Ok(Some(report))
    }

    fn entries(&self) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        // Only fully understood SR directories are eligible. Unknown content,
        // symlinks and unfinished inference directories are never recursively
        // removed. A completed entry is evicted as a unit.
        for item in fs::read_dir(&self.sr)? {
            let item = item?;
            if !item.file_type()?.is_dir() || !item.file_name().to_str().is_some_and(hex_key) { continue; }
            let mut bytes = 0;
            let mut latest = SystemTime::UNIX_EPOCH;
            let mut last_used = None;
            let mut recognized = true;
            let mut data_files = 0;
            for file in fs::read_dir(item.path())? {
                let file = file?;
                let name = file.file_name();
                if !file.file_type()?.is_file() || !matches!(name.to_str(), Some("effective.png" | "identity.json" | ".last-used" | ".in-use")) {
                    recognized = false; break;
                }
                let metadata = file.metadata()?;
                bytes += metadata.len();
                if name == ".last-used" { last_used = Some(metadata.modified()?); }
                if name == "effective.png" || name == "identity.json" {
                    latest = latest.max(metadata.modified()?); data_files += 1;
                }
            }
            if recognized && data_files > 0 {
                entries.push(Entry { path: item.path(), bytes, last_used: last_used.unwrap_or(latest).max(latest), directory: true });
            }
        }
        for (folder, extension) in [("pages", "json"), ("masks", "png")] {
            let dir = self.display.join(folder);
            if !plain_dir(&dir) { continue; }
            for item in fs::read_dir(dir)? {
                let item = item?;
                let path = item.path();
                if !item.file_type()?.is_file() || path.extension().and_then(|s| s.to_str()) != Some(extension)
                    || !path.file_stem().and_then(|s| s.to_str()).is_some_and(hex_key) { continue; }
                let metadata = item.metadata()?;
                entries.push(Entry { path, bytes: metadata.len(), last_used: metadata.modified()?, directory: false });
            }
        }
        Ok(entries)
    }

    // Caller must hold both exclusive root locks and the local write gate.
    fn sweep(&self, now: SystemTime, limit: u64, low_water: u64) -> Result<Report> {
        let mut entries = self.entries()?;
        entries.sort_by(|a, b| a.last_used.cmp(&b.last_used).then_with(|| a.path.cmp(&b.path)));
        let total = entries.iter().map(|e| e.bytes).sum();
        let mut report = Report { bytes_before: total, bytes_after: total, ..Report::default() };
        let mut capacity_eviction = false;
        for entry in entries {
            let expired = now.duration_since(entry.last_used).unwrap_or_default() >= MAX_AGE;
            // Expiration is applied first; only then trim an overfull cache to
            // 4.5 GB so every following write does not cause another eviction.
            if !expired && report.bytes_after > limit { capacity_eviction = true; }
            if !expired && !(capacity_eviction && report.bytes_after > low_water) { continue; }
            let pin = if entry.directory {
                match open_lock(&entry.path.join(".in-use")) {
                    Ok(file) => match try_lock(&file, true) {
                        Ok(true) => Some(file),
                        Ok(false) => { report.protected_entries += 1; continue; },
                        Err(_) => { report.failed_entries += 1; continue; },
                    },
                    Err(_) => { report.failed_entries += 1; continue; },
                }
            } else { None };
            let removed = if entry.directory { fs::remove_dir_all(&entry.path) } else { fs::remove_file(&entry.path) };
            drop(pin);
            if removed.is_ok() {
                report.bytes_after = report.bytes_after.saturating_sub(entry.bytes);
                report.removed_entries += 1;
                if expired { report.expired_entries += 1; }
            } else { report.failed_entries += 1; }
        }
        report.over_limit = report.bytes_after > limit;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn setup() -> (tempfile::TempDir, Arc<Manager>) {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(&dir.path().join("super-resolution-v1"), &dir.path().join("reading-native-v1")).unwrap();
        (dir, manager)
    }
    fn age(path: &Path, days: u64) {
        File::open(path).unwrap().set_times(FileTimes::new()
            .set_modified(SystemTime::now() - Duration::from_secs(days * 86400))).unwrap();
    }
    fn sr(manager: &Manager, id: u32, bytes: usize, days: u64) -> PathBuf {
        let dir = manager.sr.join(format!("{id:064x}"));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("effective.png");
        fs::write(&path, vec![1; bytes]).unwrap();
        age(&path, days);
        path
    }
    fn display(manager: &Manager, id: u32, bytes: usize, days: u64) -> PathBuf {
        let dir = manager.display.join("pages");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id:064x}.json"));
        fs::write(&path, vec![2; bytes]).unwrap();
        age(&path, days);
        path
    }
    fn sweep(manager: &Manager, limit: u64, low: u64) -> Report {
        let _files = manager.locks(true).unwrap().expect("exclusive root locks");
        manager.sweep(SystemTime::now(), limit, low).unwrap()
    }

    #[test]
    fn expire_unused_but_keep_recently_read_and_user_data() {
        let (dir, manager) = setup();
        let old = sr(&manager, 1, 10, 31);
        let used = sr(&manager, 2, 10, 90);
        touch_sr(used.parent().unwrap()).unwrap();
        let old_display = display(&manager, 3, 10, 31);
        let used_display = display(&manager, 4, 10, 90);
        touch(&used_display).unwrap();
        let translations = dir.path().join("pages/enhanced-v1/hand-edited.json");
        fs::create_dir_all(translations.parent().unwrap()).unwrap();
        fs::write(&translations, "人工修改必须保留").unwrap();
        age(&translations, 90);
        let source = dir.path().join("original.png");
        fs::write(&source, "original").unwrap();
        age(&source, 90);
        let report = sweep(&manager, 100, 90);
        assert_eq!(report.expired_entries, 2);
        assert!(!old.exists() && !old_display.exists());
        assert!(used.exists() && used_display.exists());
        assert_eq!(fs::read_to_string(translations).unwrap(), "人工修改必须保留");
        assert_eq!(fs::read_to_string(source).unwrap(), "original");
    }

    #[test]
    fn shared_capacity_evicts_lru_and_leaves_headroom() {
        let (_dir, manager) = setup();
        let oldest = sr(&manager, 1, 40, 3);
        let middle = display(&manager, 2, 40, 2);
        let newest = sr(&manager, 3, 40, 1);
        let report = sweep(&manager, 100, 90);
        assert_eq!((report.bytes_before, report.bytes_after), (120, 80));
        assert!(!oldest.exists() && middle.exists() && newest.exists());
        assert_eq!(report.expired_entries, 0);
    }

    #[test]
    fn visible_entry_survives_and_old_reply_cannot_replace_pin() {
        let (_dir, manager) = setup();
        let first = sr(&manager, 1, 100, 60);
        let second = sr(&manager, 2, 100, 60);
        let older = manager.view_ticket();
        let newer = manager.view_ticket();
        manager.pin_view(newer, &second).unwrap();
        manager.pin_view(older, &first).unwrap();
        let report = sweep(&manager, 50, 45);
        assert!(!first.exists() && second.exists());
        assert!(report.over_limit);
        assert_eq!(report.protected_entries, 1);
        manager.pin_view(manager.view_ticket(), Path::new("/original.png")).unwrap();
        assert_eq!(sweep(&manager, 50, 45).bytes_after, 0);
        assert!(!second.exists());
    }

    #[test]
    fn unknown_files_and_symlinks_never_expand_deletion_scope() {
        let (dir, manager) = setup();
        let unknown = sr(&manager, 1, 10, 60);
        fs::write(unknown.parent().unwrap().join("user-notes.txt"), "keep").unwrap();
        let external = dir.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("effective.png"), "keep-original").unwrap();
        symlink(&external, manager.sr.join(format!("{:064x}", 2))).unwrap();
        let linked_file = sr(&manager, 3, 10, 60);
        fs::remove_file(&linked_file).unwrap();
        symlink(external.join("effective.png"), &linked_file).unwrap();
        let valid = display(&manager, 4, 10, 60);
        let report = sweep(&manager, 1, 0);
        assert_eq!(report.removed_entries, 1);
        assert!(!valid.exists() && unknown.exists() && linked_file.exists());
        assert_eq!(fs::read_to_string(external.join("effective.png")).unwrap(), "keep-original");
    }

    #[tokio::test]
    async fn activity_and_other_runtime_pins_block_eviction() {
        let (_dir, manager) = setup();
        let other = Manager::new(&manager.sr, &manager.display).unwrap();
        let file = sr(&manager, 1, 10, 60);
        let activity = other.activity().await.unwrap();
        assert!(manager.maintain().await.unwrap().is_none());
        other.pin_view(other.view_ticket(), &file).unwrap();
        drop(activity);
        let report = manager.maintain().await.unwrap().unwrap();
        assert_eq!(report.protected_entries, 1);
        assert!(file.exists());
        drop(other);
        let report = manager.maintain().await.unwrap().unwrap();
        assert_eq!(report.removed_entries, 1);
        assert!(!file.exists());
    }

    #[test]
    fn exact_limit_and_future_timestamps_do_not_trigger_eviction() {
        let (_dir, manager) = setup();
        let path = sr(&manager, 1, 100, 0);
        File::open(&path).unwrap().set_times(FileTimes::new()
            .set_modified(SystemTime::now() + Duration::from_secs(86400))).unwrap();
        assert_eq!(sweep(&manager, 100, 90).removed_entries, 0);
        assert!(path.exists());
    }
}
