/// Session storage — JSON file-based conversation history.
/// Persists each turn as a separate JSON file in ~/.hermes-voicedesk/history/

use chrono::Utc;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConversationTurn {
    pub id: String,
    pub user_text: String,
    pub ai_text: String,
    pub timestamp: String,
    pub session_id: String,
}

/// Get the history directory: ~/.hermes-voicedesk/history/
fn history_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".hermes-voicedesk").join("history")
}

/// Create a new conversation turn with auto-generated ID and timestamp.
pub fn new_turn(session_id: &str, user_text: &str, ai_text: &str) -> ConversationTurn {
    let now = Utc::now();
    ConversationTurn {
        id: format!("turn-{}", now.timestamp_millis()),
        user_text: user_text.to_string(),
        ai_text: ai_text.to_string(),
        timestamp: now.to_rfc3339(),
        session_id: session_id.to_string(),
    }
}

/// Save a conversation turn to a JSON file.
pub fn save_turn(turn: &ConversationTurn) -> Result<(), String> {
    let dir = history_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create history directory: {}", e))?;

    let filename = format!("{}.json", turn.id);
    let path = dir.join(&filename);
    let json =
        serde_json::to_string_pretty(turn).map_err(|e| format!("Failed to serialize turn: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write turn file: {}", e))?;

    log::info!("Saved conversation turn: {} (session: {})", turn.id, turn.session_id);
    Ok(())
}

/// Load all conversation turns for a session, sorted by timestamp.
pub fn load_history(session_id: &str) -> Result<Vec<ConversationTurn>, String> {
    let dir = history_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut turns: Vec<ConversationTurn> = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("Failed to read history directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "json") {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<ConversationTurn>(&content) {
                    Ok(turn) if turn.session_id == session_id => turns.push(turn),
                    Ok(_) => {} // different session, skip
                    Err(e) => log::warn!("Failed to parse turn file {:?}: {}", path, e),
                },
                Err(e) => log::warn!("Failed to read turn file {:?}: {}", path, e),
            }
        }
    }

    turns.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    log::info!(
        "Loaded {} conversation turns for session: {}",
        turns.len(),
        session_id
    );
    Ok(turns)
}
