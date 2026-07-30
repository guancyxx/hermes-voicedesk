/// Hermes Agent API client.
/// Communicates with Hermes API server at 127.0.0.1:8642.

use reqwest::Client;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

const HERMES_API_BASE: &str = "http://127.0.0.1:8642";
const HERMES_API_KEY: &str = "shujietai-dev-key-2026";

/// JARVIS persona injected as system prompt on every request.
const JARVIS_INSTRUCTIONS: &str = include_str!("../resources/jarvis-persona.md");

/// Build a reqwest client that bypasses system proxy (critical for localhost).
fn api_client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| format!("client build: {}", e))
}

/// Health check — returns true if Hermes API is reachable.
pub async fn check_health() -> Result<bool, String> {
    let client = api_client()?;
    let url = format!("{}/v1/models", HERMES_API_BASE);
    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", HERMES_API_KEY))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            log::info!("Hermes API check: {}", status);
            Ok(status.is_success())
        }
        Err(e) => {
            log::warn!("Hermes API HTTP check failed: {}", e);
            Err(format!("{}", e))
        }
    }
}

/// Send a chat message and get the full response.
pub async fn chat(message: &str, session_id: Option<&str>) -> Result<String, String> {
    let client = api_client()?;
    let mut payload = serde_json::json!({
        "input": message,
        "instructions": JARVIS_INSTRUCTIONS,
    });
    if let Some(sid) = session_id {
        payload["session_id"] = serde_json::Value::String(sid.to_string());
    }

    let resp = client
        .post(format!("{}/v1/runs", HERMES_API_BASE))
        .header("Authorization", format!("Bearer {}", HERMES_API_KEY))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("API error: {}", e))?;

    if resp.status() != 202 {
        return Err(format!("Hermes start error: {}", resp.status()));
    }

    let run_id = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("Parse error: {}", e))?
        .get("run_id")
        .and_then(|v| v.as_str())
        .ok_or("No run_id in response")?
        .to_string();

    let mut full_response = String::new();
    let mut stream = client
        .get(format!("{}/v1/runs/{}/events", HERMES_API_BASE, run_id))
        .header("Authorization", format!("Bearer {}", HERMES_API_KEY))
        .send()
        .await
        .map_err(|e| format!("Stream error: {}", e))?;

    use futures::StreamExt;
    while let Some(chunk) = stream.chunk().await.map_err(|e| format!("Chunk error: {}", e))? {
        let text = String::from_utf8_lossy(&chunk);
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(event) = serde_json::from_str::<Value>(data) {
                    if event.get("event").and_then(|v| v.as_str()) == Some("message.delta") {
                        if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                            full_response.push_str(delta);
                        }
                    }
                }
            }
        }
    }

    Ok(full_response)
}

/// Stream chat response to the frontend via Tauri events.
pub async fn chat_stream(
    message: &str,
    session_id: Option<&str>,
    app: AppHandle,
) -> Result<(), String> {
    let client = api_client()?;
    let mut payload = serde_json::json!({
        "input": message,
        "instructions": JARVIS_INSTRUCTIONS,
    });
    if let Some(sid) = session_id {
        payload["session_id"] = serde_json::Value::String(sid.to_string());
    }

    let resp = client
        .post(format!("{}/v1/runs", HERMES_API_BASE))
        .header("Authorization", format!("Bearer {}", HERMES_API_KEY))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("API error: {}", e))?;

    if resp.status() != 202 {
        return Err(format!("Hermes start error: {}", resp.status()));
    }

    let run_id = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("Parse error: {}", e))?
        .get("run_id")
        .and_then(|v| v.as_str())
        .ok_or("No run_id")?
        .to_string();

    let mut stream = client
        .get(format!("{}/v1/runs/{}/events", HERMES_API_BASE, run_id))
        .header("Authorization", format!("Bearer {}", HERMES_API_KEY))
        .send()
        .await
        .map_err(|e| format!("Stream error: {}", e))?;

    use futures::StreamExt;
    while let Some(chunk) = stream.chunk().await.map_err(|e| format!("Chunk error: {}", e))? {
        let text = String::from_utf8_lossy(&chunk);
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(event) = serde_json::from_str::<Value>(data) {
                    let etype = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
                    match etype {
                        "message.delta" => {
                            if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                let _ = app.emit("hermes:delta", serde_json::json!({ "content": delta }));
                            }
                        }
                        "tool.started" => {
                            let _ = app.emit("hermes:tool", serde_json::json!({
                                "tool": event.get("tool").and_then(|v| v.as_str()),
                                "status": "started"
                            }));
                        }
                        "tool.completed" => {
                            let _ = app.emit("hermes:tool", serde_json::json!({
                                "tool": event.get("tool").and_then(|v| v.as_str()),
                                "status": "completed",
                                "error": event.get("error").and_then(|v| v.as_bool()).unwrap_or(false),
                            }));
                        }
                        "run.completed" => {
                            let _ = app.emit("hermes:finish", serde_json::json!({ "usage": event.get("usage") }));
                            return Ok(());
                        }
                        "run.failed" | "run.cancelled" => {
                            let _ = app.emit("hermes:error", serde_json::json!({
                                "error": event.get("error").and_then(|v| v.as_str()).unwrap_or(etype)
                            }));
                            return Err(format!("Run {}", etype));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
