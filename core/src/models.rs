use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

/// One serialized model worker. The caller owns the lifetime and job workspace.
/// Aborting an in-flight request poisons the stream; callers must discard it.
pub struct ModelWorker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    serial: u64,
    poisoned: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timed_out_worker_cannot_deliver_a_stale_reply_to_a_new_request() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let script = directory.path().join("worker.py");
        std::fs::write(
            &script,
            r#"
import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    if request['op'] == 'slow': time.sleep(2)
    print(json.dumps({'id': request['id'], 'ok': True, 'result': {'protocol': 1}}), flush=True)
"#,
        )?;
        let mut worker = ModelWorker::spawn(
            Path::new("python3"),
            &script,
            directory.path(),
            directory.path(),
        )
        .await?;
        assert!(
            worker
                .request(json!({"op": "slow"}), Duration::from_millis(20))
                .await
                .is_err()
        );
        let error = worker
            .request(json!({"op": "health"}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("interrupted"));
        Ok(())
    }
}

impl ModelWorker {
    pub async fn spawn(
        python: &Path,
        script: &Path,
        workspace: &Path,
        weights: &Path,
    ) -> Result<Self> {
        let mut child = Command::new(python)
            .arg(script)
            .args([
                "--workspace",
                &workspace.to_string_lossy(),
                "--weights",
                &weights.to_string_lossy(),
            ])
            .env_remove("KOMA_TRANSLATION_API_KEY")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("start model worker")?;
        let input = child.stdin.take().context("model stdin")?;
        let output = BufReader::new(child.stdout.take().context("model stdout")?);
        let mut worker = Self {
            child,
            input,
            output,
            serial: 0,
            poisoned: false,
        };
        let health = worker
            .request(json!({"op": "health"}), Duration::from_secs(10))
            .await?;
        if health["protocol"] != 1 {
            bail!("unsupported model protocol")
        }
        Ok(worker)
    }

    pub async fn request(&mut self, mut value: Value, timeout: Duration) -> Result<Value> {
        if self.poisoned {
            bail!("model stream was interrupted; restart worker")
        }
        self.serial += 1;
        let serial = self.serial;
        let object = value
            .as_object_mut()
            .context("model request must be an object")?;
        object.insert("id".into(), json!(serial));
        object.insert("protocol".into(), json!(1));
        let mut encoded = serde_json::to_vec(&value)?;
        encoded.push(b'\n');
        // Set before awaiting so cancellation cannot reuse a half-consumed reply.
        self.poisoned = true;
        let result = tokio::time::timeout(timeout, async {
            self.input.write_all(&encoded).await?;
            self.input.flush().await?;
            let mut line = String::new();
            if self.output.read_line(&mut line).await? == 0 {
                bail!("model process closed output")
            }
            if line.len() > 16 * 1024 * 1024 {
                bail!("model reply exceeds limit")
            }
            let reply: Value = serde_json::from_str(&line).context("invalid model reply")?;
            if reply["id"] != serial {
                bail!("model reply does not match request")
            }
            Ok::<Value, anyhow::Error>(reply)
        })
        .await
        .context("model request timed out")??;
        self.poisoned = false;
        if result["ok"] != true {
            bail!(
                "model operation failed: {}",
                result["error"].as_str().unwrap_or("unknown")
            )
        }
        Ok(result["result"].clone())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.request(json!({"op": "shutdown"}), Duration::from_secs(5))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await??;
        Ok(())
    }
}
