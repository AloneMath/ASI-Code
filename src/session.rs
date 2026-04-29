use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CHECKPOINT_FILE_NAME: &str = "_checkpoint_latest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    #[serde(default)]
    pub meta: Option<SessionMeta>,
    #[serde(default)]
    pub last_stop_reason_raw: Option<String>,
    #[serde(default)]
    pub last_stop_reason_alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfidenceGateSessionStats {
    pub checks: usize,
    pub missing_declaration: usize,
    pub low_declaration: usize,
    pub blocked_risky_toolcalls: usize,
    pub retries_exhausted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMeta {
    pub source: String,
    pub agent_enabled: bool,
    pub auto_loop_stop_reason: Option<String>,
    pub confidence_gate: ConfidenceGateSessionStats,
}

#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn default() -> Result<Self, String> {
        let root = std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join("sessions");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }

    pub fn save(
        &self,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
    ) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        self.save_with_id_and_meta(&format!("{}", ts), provider, model, messages, None)
    }

    pub fn save_with_meta(
        &self,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
        meta: Option<SessionMeta>,
    ) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        self.save_with_id_and_meta(&format!("{}", ts), provider, model, messages, meta)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn save_with_id(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
    ) -> Result<PathBuf, String> {
        self.save_with_id_and_meta(session_id, provider, model, messages, None)
    }

    pub fn save_with_id_and_meta(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
        meta: Option<SessionMeta>,
    ) -> Result<PathBuf, String> {
        self.save_with_id_and_meta_and_stop_reason(
            session_id,
            provider,
            model,
            messages,
            meta,
            None,
            None,
        )
    }

    pub fn save_with_id_and_meta_and_stop_reason(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
        meta: Option<SessionMeta>,
        last_stop_reason_raw: Option<String>,
        last_stop_reason_alias: Option<String>,
    ) -> Result<PathBuf, String> {
        let path = self.session_file_path(session_id)?;
        let payload = Session {
            session_id: session_id.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            messages,
            meta,
            last_stop_reason_raw,
            last_stop_reason_alias,
        };
        self.write_session_file(&path, &payload)?;
        Ok(path)
    }

    pub fn load(&self, session_id: &str) -> Result<Session, String> {
        let path = self.session_file_path(session_id)?;
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn list_sessions(&self, limit: usize) -> Result<Vec<String>, String> {
        let mut entries = Vec::new();
        let rd = fs::read_dir(&self.root).map_err(|e| e.to_string())?;
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = p.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            if name.eq_ignore_ascii_case(CHECKPOINT_FILE_NAME) {
                continue;
            }
            entries.push(p);
        }
        entries.sort_by_key(|p| std::cmp::Reverse(modified_millis(p).unwrap_or(0)));
        Ok(entries
            .into_iter()
            .take(limit)
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|x| x.to_str())
                    .map(|s| s.to_string())
            })
            .collect())
    }

    pub fn save_checkpoint_with_meta(
        &self,
        provider: &str,
        model: &str,
        messages: Vec<serde_json::Value>,
        meta: Option<SessionMeta>,
    ) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let payload = Session {
            session_id: format!("checkpoint-{}", ts),
            provider: provider.to_string(),
            model: model.to_string(),
            messages,
            meta,
            last_stop_reason_raw: None,
            last_stop_reason_alias: None,
        };
        let path = self.checkpoint_path();
        self.write_session_file(&path, &payload)?;
        Ok(path)
    }

    pub fn load_checkpoint(&self) -> Result<Session, String> {
        let path = self.checkpoint_path();
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn clear_checkpoint(&self) -> Result<(), String> {
        let path = self.checkpoint_path();
        if path.exists() {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn checkpoint_exists(&self) -> bool {
        self.checkpoint_path().exists()
    }

    fn checkpoint_path(&self) -> PathBuf {
        self.root.join(CHECKPOINT_FILE_NAME)
    }

    fn session_file_path(&self, session_id: &str) -> Result<PathBuf, String> {
        if !is_valid_session_id(session_id) {
            return Err("invalid session_id; allowed characters: [A-Za-z0-9_-]".to_string());
        }
        Ok(self.root.join(format!("{}.json", session_id)))
    }

    fn write_session_file(&self, path: &Path, payload: &Session) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("create_dir_all {}: {}", parent.display(), e)
            })?;
        }
        let body = serde_json::to_string_pretty(payload).map_err(|e| e.to_string())?;
        fs::write(path, body).map_err(|e| e.to_string())
    }
}

fn is_valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn modified_millis(path: &Path) -> Option<u128> {
    let md = fs::metadata(path).ok()?;
    let t = md.modified().ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checkpoint_roundtrip_and_list_excludes_checkpoint() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_test_{}", ts));
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        let messages = vec![json!({"role": "user", "content": "hello"})];

        let checkpoint_path = store
            .save_checkpoint_with_meta("openai", "gpt-4.1-mini", messages.clone(), None)
            .unwrap();
        assert!(checkpoint_path.ends_with(CHECKPOINT_FILE_NAME));
        assert!(store.checkpoint_exists());

        let checkpoint = store.load_checkpoint().unwrap();
        assert_eq!(checkpoint.provider, "openai");
        assert_eq!(checkpoint.model, "gpt-4.1-mini");
        assert_eq!(checkpoint.messages.len(), 1);

        let _normal = store.save("openai", "gpt-4.1-mini", messages).unwrap();
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.iter().all(|id| !id.contains("_checkpoint")));

        store.clear_checkpoint().unwrap();
        assert!(!store.checkpoint_exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_with_id_overwrites_same_session_file() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_overwrite_test_{}", ts));
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        let sid = "manual_1";
        let first = vec![json!({"role": "user", "content": "hello"})];
        let second = vec![json!({"role": "user", "content": "bye"})];

        let p1 = store
            .save_with_id(sid, "openai", "gpt-4.1-mini", first)
            .unwrap();
        let p2 = store
            .save_with_id(sid, "openai", "gpt-4.1-mini", second)
            .unwrap();
        assert_eq!(p1, p2);

        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded.session_id, sid);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(
            loaded.messages[0].get("content").and_then(|v| v.as_str()),
            Some("bye")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_with_meta_roundtrip_persists_confidence_gate_stats() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_meta_test_{}", ts));
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        let sid = "meta_1";
        let msgs = vec![json!({"role": "assistant", "content": "ok"})];
        let meta = SessionMeta {
            source: "prompt".to_string(),
            agent_enabled: true,
            auto_loop_stop_reason: Some("none".to_string()),
            confidence_gate: ConfidenceGateSessionStats {
                checks: 3,
                missing_declaration: 1,
                low_declaration: 1,
                blocked_risky_toolcalls: 2,
                retries_exhausted: 0,
            },
        };

        store
            .save_with_id_and_meta(sid, "openai", "gpt-4.1-mini", msgs, Some(meta))
            .unwrap();
        let loaded = store.load(sid).unwrap();
        let got = loaded.meta.unwrap();
        assert_eq!(got.source, "prompt");
        assert!(got.agent_enabled);
        assert_eq!(got.confidence_gate.checks, 3);
        assert_eq!(got.confidence_gate.blocked_risky_toolcalls, 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deserialize_legacy_session_without_meta() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_legacy_test_{}", ts));
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore { root: root.clone() };
        let path = root.join("legacy_1.json");
        let body = r#"{
  "session_id": "legacy_1",
  "provider": "openai",
  "model": "gpt-4.1-mini",
  "messages": [{"role":"user","content":"hello"}]
}"#;
        fs::write(&path, body).unwrap();

        let loaded = store.load("legacy_1").unwrap();
        assert_eq!(loaded.session_id, "legacy_1");
        assert!(loaded.meta.is_none());
        assert!(loaded.last_stop_reason_raw.is_none());
        assert!(loaded.last_stop_reason_alias.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_with_stop_reason_roundtrip() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_stop_reason_test_{}", ts));
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        let sid = "stop_1";
        let msgs = vec![json!({"role": "assistant", "content": "ok"})];

        store
            .save_with_id_and_meta_and_stop_reason(
                sid,
                "openai",
                "gpt-5.3-codex",
                msgs,
                None,
                Some("stop".to_string()),
                Some("completed".to_string()),
            )
            .unwrap();

        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded.last_stop_reason_raw.as_deref(), Some("stop"));
        assert_eq!(loaded.last_stop_reason_alias.as_deref(), Some("completed"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reject_invalid_session_id() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!("asi_session_store_invalid_id_test_{}", ts));
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        let res = store.load("../bad");
        assert!(res.is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_recreates_missing_sessions_directory() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let base = std::env::temp_dir().join(format!("asi_session_store_missing_dir_test_{}", ts));
        let root = base.join("sessions");
        fs::create_dir_all(&root).unwrap();

        let store = SessionStore { root: root.clone() };
        fs::remove_dir_all(&root).unwrap();
        assert!(!root.exists());

        let path = store
            .save_with_id("recreate_1", "openai", "gpt-4.1-mini", vec![json!({"role":"user","content":"hello"})])
            .expect("save should recreate missing sessions directory");
        assert!(path.exists());
        assert!(root.exists());

        let loaded = store.load("recreate_1").unwrap();
        assert_eq!(loaded.session_id, "recreate_1");

        let _ = fs::remove_dir_all(base);
    }
}
