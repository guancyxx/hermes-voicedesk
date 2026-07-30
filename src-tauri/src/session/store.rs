/// Session storage — placeholder for SQLite-backed conversation history.
/// Will use tauri-plugin-sql for persistence.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConversationTurn {
    pub id: String,
    pub user_text: String,
    pub ai_text: String,
    pub timestamp: String,
    pub session_id: String,
}

/// Save a conversation turn. Placeholder for SQLite implementation.
pub fn save_turn(_turn: &ConversationTurn) -> Result<(), String> {
    // TODO: Implement with tauri-plugin-sql
    Ok(())
}

/// Load conversation history for a session.
pub fn load_history(_session_id: &str) -> Result<Vec<ConversationTurn>, String> {
    // TODO: Implement with tauri-plugin-sql
    Ok(Vec::new())
}
