//! A separate budget for page OCR/translation JSON, with conservative retention
//! of manual and legacy data. Only pages created with automatic provenance can
//! be evicted; absence of an edit flag is not proof that an old page is machine-only.
use crate::cache_policy::{Report, hex_key, open_lock, plain_dir, try_lock};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{fs::{self, File, OpenOptions}, io::Read, os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf}, time::{Duration, SystemTime}};

pub const LIMIT_BYTES: u64 = 500_000_000;
pub const LOW_WATER_BYTES: u64 = 450_000_000;
const MAX_AGE: Duration = Duration::from_secs(30 * 86400);
// Oversized/corrupt/unknown records are preserved, not parsed without a bound.
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

pub fn mark_automatic(page: &mut Value) {
    page["cache_retention"] = json!({"version": 1, "kind": "automatic"});
}
pub fn mark_manual(page: &mut Value) {
    page["cache_retention"] = json!({"version": 1, "kind": "manual"});
}

fn automatic(page: &Value) -> bool {
    page["version"] == 2 && page["width"].as_u64().is_some_and(|n| n > 0)
        && page["height"].as_u64().is_some_and(|n| n > 0)
        && page["cache_retention"] == json!({"version": 1, "kind": "automatic"})
        && page["blocks"].as_array().is_some_and(|blocks| blocks.iter().all(|b|
            b.is_object() && b["source"].is_string() && b["translation"].is_string()
            && b["edited"] == false))
}

fn may_evict(path: &Path) -> bool {
    let read = || -> Result<Value> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
        ensure!(file.metadata()?.len() <= MAX_RECORD_BYTES, "oversized record");
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_RECORD_BYTES, "oversized record");
        Ok(serde_json::from_slice(&bytes)?)
    };
    read().is_ok_and(|page| automatic(&page))
}

struct Entry { path: PathBuf, bytes: u64, used: SystemTime }
pub struct Store { pub root: PathBuf }

impl Store {
    // Allow only the four layouts actually produced by Runtime::cache_for.
    fn known_path(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else { return false; };
        let parts: Option<Vec<_>> = relative.iter().map(|s| s.to_str()).collect();
        let Some(parts) = parts else { return false; };
        let Some(file) = parts.last() else { return false; };
        if !file.strip_suffix(".json").is_some_and(hex_key) { return false; }
        match &parts[..parts.len()-1] {
            [] | ["vision-v1"] => true,
            ["enhanced-v1", key] | ["enhanced-v1", key, "vision-v1"] => hex_key(key),
            _ => false,
        }
    }

    fn pin_path(path: &Path) -> PathBuf { path.with_extension("json.pin") }

    // Caller holds Activity. Stable sidecar lock survives atomic JSON rewrites.
    pub fn pin(&self, path: &Path) -> Result<File> {
        ensure!(self.known_path(path), "无效译文缓存路径");
        let parent = path.parent().unwrap();
        let mut directory = self.root.clone();
        for component in parent.strip_prefix(&self.root)?.components() {
            directory.push(component);
            if directory.exists() { ensure!(plain_dir(&directory), "译文缓存目录不能是符号链接"); }
            else {
                match fs::create_dir(&directory) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        ensure!(plain_dir(&directory), "译文缓存目录不能是符号链接");
                    },
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let file = open_lock(&Self::pin_path(path))?;
        ensure!(file.metadata()?.len() == 0, "译文缓存锁含未知内容，已保留");
        ensure!(try_lock(&file, false)?, "译文缓存正在清理，请重试");
        Ok(file)
    }

    fn directories(&self) -> Result<Vec<PathBuf>> {
        let mut directories = vec![self.root.clone()];
        let vision = self.root.join("vision-v1");
        if plain_dir(&vision) { directories.push(vision); }
        let enhanced = self.root.join("enhanced-v1");
        if plain_dir(&enhanced) {
            for item in fs::read_dir(enhanced)? {
                let item = item?;
                if !item.file_type()?.is_dir() || !item.file_name().to_str().is_some_and(hex_key) { continue; }
                let path = item.path();
                directories.push(path.clone());
                let vision = path.join("vision-v1");
                if plain_dir(&vision) { directories.push(vision); }
            }
        }
        Ok(directories)
    }

    // Caller holds all exclusive root locks. Never recurse into unknown folders.
    pub fn sweep(&self, now: SystemTime, limit: u64, low_water: u64) -> Result<Report> {
        let directories = self.directories()?;
        let mut entries = Vec::new();
        for directory in &directories {
            for item in fs::read_dir(directory)? {
                let item = item?;
                if !item.file_type()?.is_file() { continue; }
                let path = item.path();
                if self.known_path(&path) {
                    let metadata = item.metadata()?;
                    entries.push(Entry { path, bytes: metadata.len(), used: metadata.modified()? });
                } else if path.extension().is_some_and(|e| e == "pin") {
                    let page = path.with_extension("");
                    // Reclaim unused zero-length locks from pages opened but
                    // never processed. A live viewer holds the shared lock.
                    if self.known_path(&page) && !page.exists() && item.metadata()?.len() == 0 {
                        if let Ok(pin) = open_lock(&path) {
                            if try_lock(&pin, true).unwrap_or(false) { let _ = fs::remove_file(path); }
                        }
                    }
                }
            }
        }
        entries.sort_by(|a, b| a.used.cmp(&b.used).then_with(|| a.path.cmp(&b.path)));
        let total = entries.iter().map(|e| e.bytes).sum();
        let mut report = Report { bytes_before: total, bytes_after: total, ..Report::default() };
        let mut capacity_eviction = false;
        for entry in entries {
            let expired = now.duration_since(entry.used).unwrap_or_default() >= MAX_AGE;
            if !expired && report.bytes_after > limit { capacity_eviction = true; }
            if !expired && !(capacity_eviction && report.bytes_after > low_water) { continue; }
            if !may_evict(&entry.path) { report.protected_entries += 1; continue; }
            let pin = match open_lock(&Self::pin_path(&entry.path)) {
                Ok(pin) => pin, Err(_) => { report.failed_entries += 1; continue; },
            };
            if pin.metadata()?.len() != 0 { report.protected_entries += 1; continue; }
            match try_lock(&pin, true) {
                Ok(true) => (),
                Ok(false) => { report.protected_entries += 1; continue; },
                Err(_) => { report.failed_entries += 1; continue; },
            }
            if fs::remove_file(&entry.path).is_ok() {
                report.bytes_after = report.bytes_after.saturating_sub(entry.bytes);
                report.removed_entries += 1;
                if expired { report.expired_entries += 1; }
                let _ = fs::remove_file(Self::pin_path(&entry.path));
            } else { report.failed_entries += 1; }
        }
        // Only remove empty, known namespace directories. The root stays put.
        for directory in directories.iter().rev().filter(|p| **p != self.root) {
            let _ = fs::remove_dir(directory);
        }
        report.over_limit = report.bytes_after > limit;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::FileTimes, os::unix::fs::symlink};

    fn page() -> Value {
        let mut page = json!({"version":2,"width":100,"height":200,"blocks":[{
            "id":"1","source":"原文","translation":"自动译文","edited":false}]});
        mark_automatic(&mut page);
        page
    }
    fn save(root: &Path, relative: &str, page: &Value, days: u64) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(page).unwrap()).unwrap();
        File::open(&path).unwrap().set_times(FileTimes::new().set_modified(
            SystemTime::now()-Duration::from_secs(days*86400))).unwrap();
        path
    }

    #[test]
    fn expires_only_automatic_pages_in_all_four_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {root:dir.path().to_owned()};
        let key = format!("{:064x}",1);
        let names = [format!("{key}.json"),format!("vision-v1/{key}.json"),
            format!("enhanced-v1/{key}/{key}.json"),format!("enhanced-v1/{key}/vision-v1/{key}.json")];
        let files:Vec<_> = names.iter().map(|n|save(&store.root,n,&page(),31)).collect();
        let report=store.sweep(SystemTime::now(),100_000,90_000).unwrap();
        assert_eq!(report.expired_entries,4);
        assert!(files.iter().all(|p|!p.exists()));
    }

    #[test]
    fn preserves_manual_source_edits_legacy_corruption_and_external_symlinks() {
        let dir=tempfile::tempdir().unwrap();
        let store=Store{root:dir.path().join("pages")};fs::create_dir(&store.root).unwrap();
        let mut manual=page();mark_manual(&mut manual); // edited=false still protects a source correction.
        let mut legacy=page();legacy.as_object_mut().unwrap().remove("cache_retention");
        let mut edited=page();edited["blocks"][0]["edited"]=json!(true);
        let pages=[manual,legacy,edited];
        let files:Vec<_>=pages.iter().enumerate().map(|(i,p)|save(&store.root,&format!("{i:064x}.json"),p,90)).collect();
        let corrupt=store.root.join(format!("{:064x}.json",4));fs::write(&corrupt,"{broken").unwrap();
        let external=save(dir.path(),"original.json",&page(),90);
        let link=store.root.join(format!("{:064x}.json",5));symlink(&external,&link).unwrap();
        symlink(dir.path(),store.root.join("vision-v1")).unwrap();
        let bytes:Vec<_>=files.iter().map(|p|fs::read(p).unwrap()).collect();
        let report=store.sweep(SystemTime::now(),1,0).unwrap();
        assert_eq!(report.removed_entries,0);assert!(report.over_limit);
        for (path,bytes) in files.iter().zip(bytes) {assert_eq!(fs::read(path).unwrap(),bytes);}
        assert!(corrupt.exists()&&external.exists()&&link.exists());
    }

    #[test]
    fn separate_budget_lru_renewal_and_visible_page_survive_atomic_write() {
        let dir=tempfile::tempdir().unwrap();let store=Store{root:dir.path().to_owned()};
        let first=save(&store.root,&format!("{:064x}.json",1),&page(),3);
        let second=save(&store.root,&format!("{:064x}.json",2),&page(),2);
        let third=save(&store.root,&format!("{:064x}.json",3),&page(),1);
        let size=fs::metadata(&first).unwrap().len();
        let report=store.sweep(SystemTime::now(),size*2,size*2).unwrap();
        assert_eq!(report.removed_entries,1);assert!(!first.exists()&&second.exists()&&third.exists());
        crate::cache_policy::touch(&second).unwrap();
        let pin=store.pin(&third).unwrap();
        crate::persistence::atomic_json(&third,&page()).unwrap();
        let future=SystemTime::now()+Duration::from_secs(31*86400);
        let report=store.sweep(future,0,0).unwrap();
        assert!(!second.exists()&&third.exists());assert_eq!(report.protected_entries,1);
        drop(pin);
        store.sweep(future,0,0).unwrap();assert!(!third.exists());
    }

    #[test]
    fn clears_unused_pins_but_keeps_live_empty_page_pin() {
        let dir=tempfile::tempdir().unwrap();let store=Store{root:dir.path().to_owned()};
        let path=store.root.join(format!("vision-v1/{:064x}.json",1));
        let pin=store.pin(&path).unwrap();
        store.sweep(SystemTime::now(),1,0).unwrap();assert!(Store::pin_path(&path).exists());
        drop(pin);
        store.sweep(SystemTime::now(),1,0).unwrap();assert!(!Store::pin_path(&path).exists());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(Store::pin_path(&path),"unknown user content").unwrap();
        store.sweep(SystemTime::now(),1,0).unwrap();
        assert_eq!(fs::read_to_string(Store::pin_path(&path)).unwrap(),"unknown user content");
        assert!(store.pin(&path).is_err());
        assert!(store.pin(&store.root.join("../outside.json")).is_err());
    }
}
