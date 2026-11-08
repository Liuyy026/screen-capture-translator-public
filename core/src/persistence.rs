//! Atomic private page/status persistence shared by CLI and queued tasks.
use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), value)?;
    temp.as_file_mut().flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
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
