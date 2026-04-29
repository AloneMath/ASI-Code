use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::agentd_root;

static NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceIdentityRecord {
    device_id: String,
    token: String,
    created_at: u64,
    rotated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceIdentitySummary {
    pub device_id: String,
    pub created_at: u64,
    pub rotated_at: u64,
    pub token_present: bool,
}

#[derive(Debug, Default)]
pub struct DeviceIdentity;

impl DeviceIdentity {
    pub fn ensure_registered(&self) -> Result<DeviceIdentitySummary, String> {
        let path = identity_file()?;
        let record = if path.exists() {
            load_identity(&path)?
        } else {
            let now = now_secs();
            let record = DeviceIdentityRecord {
                device_id: make_device_id(),
                token: make_token(),
                created_at: now,
                rotated_at: now,
            };
            save_identity(&path, &record)?;
            record
        };

        Ok(summary_from_record(&record))
    }

    pub fn summary(&self) -> Result<DeviceIdentitySummary, String> {
        self.ensure_registered()
    }
}

fn summary_from_record(record: &DeviceIdentityRecord) -> DeviceIdentitySummary {
    DeviceIdentitySummary {
        device_id: record.device_id.clone(),
        created_at: record.created_at,
        rotated_at: record.rotated_at,
        token_present: !record.token.trim().is_empty(),
    }
}

fn identity_file() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("device_identity.json"))
}

fn load_identity(path: &PathBuf) -> Result<DeviceIdentityRecord, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid identity {}: {}", path.display(), e))
}

fn save_identity(path: &PathBuf, record: &DeviceIdentityRecord) -> Result<(), String> {
    let body = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn make_device_id() -> String {
    let a = hash_fragment("device_id_a");
    let b = hash_fragment("device_id_b");
    format!("dev-{}-{}", &a[..8], &b[..8])
}

fn make_token() -> String {
    let a = hash_fragment("token_a");
    let b = hash_fragment("token_b");
    let c = hash_fragment("token_c");
    format!("{}{}{}", a, b, c)
}

fn hash_fragment(label: &str) -> String {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();

    let mut h = DefaultHasher::new();
    format!("{}:{}:{}:{}", label, now_nanos, nonce, pid).hash(&mut h);
    format!("{:016x}", h.finish())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
