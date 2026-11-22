use super::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

// Every request must stay on this endpoint with its original key and prefix.
async fn mock(
    provider: &str,
    replies: Vec<(&'static str, u16, Value)>,
) -> Result<(TranslationService, JoinHandle<Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/proxy", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let mut held_connections = Vec::new();
        for (expected, status, response) in replies {
            let (mut stream, _) = listener.accept().await?;
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            assert_eq!(line.trim_end(), format!("{expected} HTTP/1.1"));
            let mut authorized = false;
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).await?;
                if line.trim().is_empty() {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("authorization") {
                    authorized = value.trim() == "Bearer test-key";
                }
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse()?;
                }
            }
            assert!(authorized);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await?;
            if status == 0 {
                // Accept this request, then serve independent status queries
                // without delivering its response before the client timeout.
                held_connections.push(stream);
                continue;
            }
            if expected.ends_with("/api/generate") {
                let payload: Value = serde_json::from_slice(&body)?;
                assert_eq!(
                    payload,
                    json!({"model":"reader", "keep_alive":0, "stream":false})
                );
            }
            let body = response.to_string();
            let redirect = if status == 307 {
                "Location: http://127.0.0.1:1/elsewhere\r\n"
            } else {
                ""
            };
            let output = format!(
                "HTTP/1.1 {status} Mock\r\n{redirect}Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(output.as_bytes()).await?;
        }
        Ok(())
    });
    Ok((
        TranslationService::new(provider, Some(&base), Some("test-key"))?,
        server,
    ))
}

const STATUS: &str = "GET /proxy/v1/models/status";
const OMLX_POST: &str = "POST /proxy/v1/models/real%20%231%25%3F/unload";
const PHYSICAL: &str = "real #1%?";

fn omlx_status(loaded: bool, loading: bool) -> Value {
    json!({"models":[
        {"id":PHYSICAL, "model_alias":"friendly", "loaded":loaded, "is_loading":loading},
        {"id":"profile/reader", "source_model_id":PHYSICAL, "loaded":loaded, "is_loading":loading},
        {"id":"other", "loaded":true, "is_loading":false}
    ]})
}

#[tokio::test]
async fn profile_waits_for_load_then_checks_queued_unload() -> Result<()> {
    let (service, server) = mock(
        "omlx",
        vec![
            (STATUS, 200, omlx_status(false, true)),
            (STATUS, 200, omlx_status(true, false)),
            (
                OMLX_POST,
                202,
                json!({"status":"unloading", "model_id":PHYSICAL}),
            ),
            (STATUS, 200, omlx_status(true, false)),
            (STATUS, 200, omlx_status(false, false)),
        ],
    )
    .await?;
    service.unload_model("profile/reader").await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn healthy_cold_load_can_outlast_a_single_http_timeout() -> Result<()> {
    let mut replies = vec![(STATUS, 200, omlx_status(false, true)); 26];
    replies.push((STATUS, 200, omlx_status(true, false)));
    replies.push((OMLX_POST, 200, json!({"model_id": PHYSICAL, "status":"ok"})));
    let (service, server) = mock("omlx", replies).await?;
    service.unload_model(PHYSICAL).await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn unresponsive_server_fails_before_the_release_deadline() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let service = TranslationService::new("omlx", Some(&url), Some(""))?;
    let error = tokio::time::timeout(
        HTTP_TIMEOUT + Duration::from_secs(3),
        service.unload_model("reader"),
    )
    .await?
    .unwrap_err();
    assert!(error.to_string().contains("无法连接"));
    Ok(())
}

#[tokio::test]
async fn accepted_post_with_no_reply_is_confirmed_without_resubmission() -> Result<()> {
    let (service, server) = mock(
        "omlx",
        vec![
            (STATUS, 200, omlx_status(true, false)),
            (OMLX_POST, 0, Value::Null),
            (STATUS, 200, omlx_status(true, false)),
            (STATUS, 200, omlx_status(false, false)),
        ],
    )
    .await?;
    service.unload_model(PHYSICAL).await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn absent_target_ignores_another_models_pending_memory() -> Result<()> {
    let status = json!({"current_model_memory":7, "models":[
        {"id":PHYSICAL, "loaded":false, "is_loading":false, "resident_estimated_size":5},
        {"id":"other", "loaded":false, "is_loading":false, "resident_estimated_size":7}
    ]});
    let (service, server) = mock("omlx", vec![(STATUS, 200, status)]).await?;
    service.unload_model(PHYSICAL).await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn queued_unload_waits_for_target_memory_and_leaves_other_models_resident() -> Result<()> {
    let status = |loaded, memory| {
        json!({"current_model_memory":memory, "models":[
            {"id":PHYSICAL, "loaded":loaded, "is_loading":false, "resident_estimated_size":5},
            {"id":"other", "loaded":true, "is_loading":false, "resident_estimated_size":7},
            {"id":"other/profile", "source_model_id":"other", "loaded":true, "resident_estimated_size":7},
            {"id":"markdown", "source_type":"builtin", "loaded":true}
        ]})
    };
    let (service, server) = mock(
        "omlx",
        vec![
            (STATUS, 200, status(true, 12)),
            (
                OMLX_POST,
                202,
                json!({"status":"unloading", "model_id":PHYSICAL}),
            ),
            (STATUS, 200, status(false, 12)),
            (STATUS, 200, status(false, 7)),
        ],
    )
    .await?;
    service.unload_model(PHYSICAL).await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn concurrent_expiry_uses_omlx_error_shape_and_confirms_absence() -> Result<()> {
    let (service, server) = mock(
        "omlx",
        vec![
            (STATUS, 200, omlx_status(true, false)),
            (
                OMLX_POST,
                400,
                json!({"error":{"message":format!("Model not loaded: {PHYSICAL}")}}),
            ),
            (STATUS, 200, omlx_status(false, false)),
        ],
    )
    .await?;
    service.unload_model("friendly").await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn ollama_waits_for_the_selected_runner_to_disappear() -> Result<()> {
    let (service, server) = mock(
        "ollama",
        vec![
            (
                "POST /proxy/api/generate",
                200,
                json!({"done":true, "done_reason":"unload"}),
            ),
            (
                "GET /proxy/api/ps",
                200,
                json!({"models":[{"name":"reader:latest"}]}),
            ),
            (
                "GET /proxy/api/ps",
                200,
                json!({"models":[{"name":"other:latest"}]}),
            ),
        ],
    )
    .await?;
    service.unload_model("reader").await?;
    tokio::time::timeout(Duration::from_secs(1), server).await??
}

#[tokio::test]
async fn unload_rejects_redirects_and_hides_response_text() -> Result<()> {
    let (service, server) = mock(
        "omlx",
        vec![
            (STATUS, 200, omlx_status(true, false)),
            (OMLX_POST, 307, json!({"detail":"test-key"})),
        ],
    )
    .await?;
    let error = service
        .unload_model("friendly")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("307") && !error.contains("test-key"));
    tokio::time::timeout(Duration::from_secs(1), server).await??
}
