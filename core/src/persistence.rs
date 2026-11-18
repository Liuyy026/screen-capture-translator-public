//! Atomic private page/status persistence shared by CLI and queued tasks.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    atomic_bytes(path,&serde_json::to_vec_pretty(value)?)
}

pub fn atomic_bytes(path:&Path,bytes:&[u8])->Result<()>{
    write_temp(path, bytes)?.persist(path)?;
    Ok(())
}

pub fn validate_page(page: &Value) -> Result<()> {
    let dimensions_valid = ["width", "height"].iter().all(|&field|
        page[field].as_u64().is_some_and(|size| (1..=i32::MAX as u64).contains(&size)));
    ensure!(dimensions_valid, "页面缓存的图像尺寸无效");
    let blocks = page["blocks"].as_array().context("页面缓存缺少文字块列表")?;
    let mut ids = std::collections::HashSet::new();
    for block in blocks {
        let id = block["id"].as_str().filter(|id| !id.is_empty()).context("文字块缺少有效编号")?;
        ensure!(ids.insert(id), "页面缓存包含重复的文字块编号");
    }
    ensure!(page["dialogues"].is_null() || page["dialogues"].as_array()
        .is_some_and(|rows| rows.iter().all(Value::is_object)), "页面缓存的对白列表无效");
    ensure!(page["diagnostics"].is_null() || page["diagnostics"].is_object(), "页面缓存的诊断信息无效");
    Ok(())
}

pub fn parse_page(bytes: &[u8]) -> Result<Value> {
    let page = serde_json::from_slice(bytes).context("无法解析页面缓存")?;
    validate_page(&page)?;
    Ok(page)
}

// Imports can race across candidate processes. Never replace a canonical
// record that another process has already created.
pub fn seed_page(path: &Path, bytes: &[u8], page: Value) -> Result<Value> {
    match write_temp(path, bytes)?.persist_noclobber(path) {
        Ok(_) => Ok(page),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            parse_page(&fs::read(path)?)
        }
        Err(error) => Err(error.into()),
    }
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file_mut().write_all(bytes)?;
    temp.as_file_mut().flush()?;
    temp.as_file().sync_all()?;
    Ok(temp)
}

pub fn cache_path(image: &Path, root: &Path) -> Result<PathBuf> {
    Ok(root.join(format!("{:x}.json", Sha256::digest(fs::read(image)?))))
}

pub fn append_metric(path: &Path, event: &Value) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}

pub struct FileObserver<'a> {
    pub destination: &'a Path,
    pub status: Option<&'a Path>,
    pub metrics: Option<&'a Path>,
}
impl crate::translate_pipeline::Observer for FileObserver<'_> {
    fn progress(&mut self, message: &str) -> Result<()> {
        if let Some(path) = self.status {
            atomic_json(path, &serde_json::json!({"message":message}))?;
        }
        Ok(())
    }
    fn save(&mut self, page: &Value) -> Result<()> {
        atomic_json(self.destination, page)
    }
    fn metrics(&mut self, event: &Value) -> Result<()> {
        if let Some(path) = self.metrics {
            append_metric(path, event)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn replacing_page_is_atomic_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("page.json");
        atomic_json(&file, &json!({"source":"日本語","translation":"手改"})).unwrap();
        atomic_json(&file, &json!({"source":"日本語","translation":"修改"})).unwrap();
        let page: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(page["translation"], "修改");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
