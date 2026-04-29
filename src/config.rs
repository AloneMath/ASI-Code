use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub provider: String,
    pub model: String,
    pub permission_mode: String,
    pub auto_review_mode: String,
    pub auto_review_severity_threshold: String,
    pub execution_speed: String,
    pub max_turns: usize,
    pub extended_thinking: bool,
    pub markdown_render: bool,
    pub theme: String,
    pub syntax_theme: String,
    pub wallet_usd: f64,
    pub api_key_provider: Option<String>,
    pub api_key: Option<String>,
    pub telemetry_enabled: bool,
    pub telemetry_log_tool_details: bool,
    pub undercover_mode: bool,
    pub safe_shell_mode: bool,
    pub remote_policy_enabled: bool,
    pub remote_policy_url: Option<String>,
    pub feature_killswitches: HashMap<String, bool>,
    pub permission_allow_rules: Vec<String>,
    pub permission_deny_rules: Vec<String>,
    pub path_restriction_enabled: bool,
    pub additional_directories: Vec<String>,
    pub voice_enabled: bool,
    pub voice_engine: String,
    pub voice_openai_voice: String,
    pub voice_timeout_secs: u64,
    pub voice_mute: bool,
    pub voice_openai_fallback_local: bool,
    pub voice_local_soft_fail: bool,
    pub voice_ptt: bool,
    pub voice_ptt_trigger: String,
    pub voice_ptt_hotkey: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConfigPatch {
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    auto_review_mode: Option<String>,
    auto_review_severity_threshold: Option<String>,
    execution_speed: Option<String>,
    max_turns: Option<usize>,
    extended_thinking: Option<bool>,
    markdown_render: Option<bool>,
    theme: Option<String>,
    syntax_theme: Option<String>,
    wallet_usd: Option<f64>,
    api_key_provider: Option<String>,
    api_key: Option<String>,
    telemetry_enabled: Option<bool>,
    telemetry_log_tool_details: Option<bool>,
    undercover_mode: Option<bool>,
    safe_shell_mode: Option<bool>,
    remote_policy_enabled: Option<bool>,
    remote_policy_url: Option<String>,
    feature_killswitches: Option<HashMap<String, bool>>,
    permission_allow_rules: Option<Vec<String>>,
    permission_deny_rules: Option<Vec<String>>,
    path_restriction_enabled: Option<bool>,
    additional_directories: Option<Vec<String>>,
    voice_enabled: Option<bool>,
    voice_engine: Option<String>,
    voice_openai_voice: Option<String>,
    voice_timeout_secs: Option<u64>,
    voice_mute: Option<bool>,
    voice_openai_fallback_local: Option<bool>,
    voice_local_soft_fail: Option<bool>,
    voice_ptt: Option<bool>,
    voice_ptt_trigger: Option<String>,
    voice_ptt_hotkey: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4.1-mini".to_string(),
            permission_mode: "on-request".to_string(),
            auto_review_mode: "off".to_string(),
            auto_review_severity_threshold: "high".to_string(),
            execution_speed: "deep".to_string(),
            max_turns: 200,
            extended_thinking: false,
            markdown_render: true,
            theme: "dark".to_string(),
            syntax_theme: "Monokai Extended".to_string(),
            wallet_usd: 0.0,
            api_key_provider: None,
            api_key: None,
            telemetry_enabled: false,
            telemetry_log_tool_details: false,
            undercover_mode: false,
            safe_shell_mode: true,
            remote_policy_enabled: false,
            remote_policy_url: None,
            feature_killswitches: HashMap::new(),
            permission_allow_rules: Vec::new(),
            permission_deny_rules: Vec::new(),
            path_restriction_enabled: true,
            additional_directories: Vec::new(),
            voice_enabled: false,
            voice_engine: "local".to_string(),
            voice_openai_voice: "alloy".to_string(),
            voice_timeout_secs: 10,
            voice_mute: false,
            voice_openai_fallback_local: true,
            voice_local_soft_fail: true,
            voice_ptt: false,
            voice_ptt_trigger: ".".to_string(),
            voice_ptt_hotkey: "F8".to_string(),
        }
    }
}

impl AppConfig {
    pub fn path() -> PathBuf {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("config.json")
    }

    pub fn load() -> Self {
        let mut cfg = Self::default();

        let home = env::var("USERPROFILE")
            .or_else(|_| env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let candidates = [
            PathBuf::from(home).join(".asi-code.json"),
            cwd.join(".claw.json"),
            cwd.join(".asi_code.json"),
            Self::path(),
        ];

        for path in candidates {
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(patch) = serde_json::from_str::<ConfigPatch>(&text) {
                    cfg.apply_patch(patch);
                }
            }
        }

        if let Ok(v) = env::var("ASI_PROVIDER") {
            cfg.provider = v;
        }
        if let Ok(v) = env::var("ASI_MODEL") {
            cfg.model = v;
        }
        if let Ok(v) = env::var("ASI_PERMISSION_MODE") {
            cfg.permission_mode = v;
        }
        if let Ok(v) = env::var("ASI_AUTO_REVIEW_MODE") {
            cfg.auto_review_mode = normalize_auto_review_mode(&v);
        }
        if let Ok(v) = env::var("ASI_AUTO_REVIEW_SEVERITY_THRESHOLD") {
            cfg.auto_review_severity_threshold = normalize_auto_review_severity_threshold(&v);
        }
        if let Ok(v) = env::var("ASI_EXECUTION_SPEED") {
            cfg.execution_speed = normalize_execution_speed(&v);
        }
        if let Ok(v) = env::var("ASI_TELEMETRY") {
            cfg.telemetry_enabled = matches!(v.as_str(), "1" | "true" | "on");
        }
        if let Ok(v) = env::var("ASI_UNDERCOVER") {
            cfg.undercover_mode = matches!(v.as_str(), "1" | "true" | "on");
        }

        if let Ok(v) = env::var("ASI_VOICE_ENABLED") {
            cfg.voice_enabled = matches!(v.as_str(), "1" | "true" | "on");
        }
        if let Ok(v) = env::var("ASI_VOICE_ENGINE") {
            cfg.voice_engine = v;
        }
        if let Ok(v) = env::var("ASI_VOICE_OPENAI_VOICE") {
            cfg.voice_openai_voice = v;
        }
        if let Ok(v) = env::var("ASI_VOICE_TIMEOUT_SECS") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.voice_timeout_secs = n.clamp(1, 120);
            }
        }
        if let Ok(v) = env::var("ASI_VOICE_MUTE") {
            cfg.voice_mute = matches!(v.as_str(), "1" | "true" | "on");
        }
        if let Ok(v) = env::var("ASI_VOICE_OPENAI_FALLBACK_LOCAL") {
            cfg.voice_openai_fallback_local = !matches!(v.as_str(), "0" | "false" | "off");
        }
        if let Ok(v) = env::var("ASI_VOICE_LOCAL_SOFT_FAIL") {
            cfg.voice_local_soft_fail = !matches!(v.as_str(), "0" | "false" | "off");
        }
        if let Ok(v) = env::var("ASI_VOICE_PTT") {
            cfg.voice_ptt = matches!(v.as_str(), "1" | "true" | "on");
        }
        if let Ok(v) = env::var("ASI_VOICE_PTT_TRIGGER") {
            if !v.trim().is_empty() {
                cfg.voice_ptt_trigger = v.trim().to_string();
            }
        }
        if let Ok(v) = env::var("ASI_VOICE_PTT_HOTKEY") {
            if !v.trim().is_empty() {
                cfg.voice_ptt_hotkey = v.trim().to_string();
            }
        }

        cfg.execution_speed = normalize_execution_speed(&cfg.execution_speed);
        cfg.provider = normalize_provider_name(&cfg.provider);
        cfg.auto_review_mode = normalize_auto_review_mode(&cfg.auto_review_mode);
        cfg.auto_review_severity_threshold =
            normalize_auto_review_severity_threshold(&cfg.auto_review_severity_threshold);
        let (reconciled_model, _) = reconcile_model_for_provider(&cfg.provider, &cfg.model);
        cfg.model = reconciled_model;

        apply_api_key_env(&cfg);
        cfg
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::path();
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, body).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn is_feature_disabled(&self, name: &str) -> bool {
        self.feature_killswitches
            .get(name)
            .copied()
            .unwrap_or(false)
    }

    pub fn set_feature_disabled(&mut self, name: &str, disabled: bool) {
        self.feature_killswitches.insert(name.to_string(), disabled);
    }

    fn apply_patch(&mut self, patch: ConfigPatch) {
        if let Some(v) = patch.provider {
            self.provider = v;
        }
        if let Some(v) = patch.model {
            self.model = v;
        }
        if let Some(v) = patch.permission_mode {
            self.permission_mode = v;
        }
        if let Some(v) = patch.auto_review_mode {
            self.auto_review_mode = normalize_auto_review_mode(&v);
        }
        if let Some(v) = patch.auto_review_severity_threshold {
            self.auto_review_severity_threshold = normalize_auto_review_severity_threshold(&v);
        }
        if let Some(v) = patch.execution_speed {
            self.execution_speed = normalize_execution_speed(&v);
        }
        if let Some(v) = patch.max_turns {
            self.max_turns = v;
        }
        if let Some(v) = patch.extended_thinking {
            self.extended_thinking = v;
        }
        if let Some(v) = patch.markdown_render {
            self.markdown_render = v;
        }
        if let Some(v) = patch.theme {
            self.theme = v;
        }
        if let Some(v) = patch.syntax_theme {
            self.syntax_theme = v;
        }
        if let Some(v) = patch.wallet_usd {
            self.wallet_usd = v;
        }
        if let Some(v) = patch.api_key_provider {
            self.api_key_provider = Some(v);
        }
        if let Some(v) = patch.api_key {
            self.api_key = Some(v);
        }
        if let Some(v) = patch.telemetry_enabled {
            self.telemetry_enabled = v;
        }
        if let Some(v) = patch.telemetry_log_tool_details {
            self.telemetry_log_tool_details = v;
        }
        if let Some(v) = patch.undercover_mode {
            self.undercover_mode = v;
        }
        if let Some(v) = patch.safe_shell_mode {
            self.safe_shell_mode = v;
        }
        if let Some(v) = patch.remote_policy_enabled {
            self.remote_policy_enabled = v;
        }
        if let Some(v) = patch.remote_policy_url {
            self.remote_policy_url = Some(v);
        }
        if let Some(v) = patch.feature_killswitches {
            for (k, disabled) in v {
                self.feature_killswitches.insert(k, disabled);
            }
        }
        if let Some(v) = patch.permission_allow_rules {
            self.permission_allow_rules = v;
        }
        if let Some(v) = patch.permission_deny_rules {
            self.permission_deny_rules = v;
        }
        if let Some(v) = patch.path_restriction_enabled {
            self.path_restriction_enabled = v;
        }
        if let Some(v) = patch.additional_directories {
            self.additional_directories = v;
        }
        if let Some(v) = patch.voice_enabled {
            self.voice_enabled = v;
        }
        if let Some(v) = patch.voice_engine {
            self.voice_engine = v;
        }
        if let Some(v) = patch.voice_openai_voice {
            self.voice_openai_voice = v;
        }
        if let Some(v) = patch.voice_timeout_secs {
            self.voice_timeout_secs = v.clamp(1, 120);
        }
        if let Some(v) = patch.voice_mute {
            self.voice_mute = v;
        }
        if let Some(v) = patch.voice_openai_fallback_local {
            self.voice_openai_fallback_local = v;
        }
        if let Some(v) = patch.voice_local_soft_fail {
            self.voice_local_soft_fail = v;
        }
        if let Some(v) = patch.voice_ptt {
            self.voice_ptt = v;
        }
        if let Some(v) = patch.voice_ptt_trigger {
            if !v.trim().is_empty() {
                self.voice_ptt_trigger = v.trim().to_string();
            }
        }
        if let Some(v) = patch.voice_ptt_hotkey {
            if !v.trim().is_empty() {
                self.voice_ptt_hotkey = v.trim().to_string();
            }
        }
    }
}

pub fn normalize_provider_name(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "openai" | "gpt" => "openai".to_string(),
        "deepseek" | "deepseek-ai" => "deepseek".to_string(),
        "claude" | "anthropic" | "claude-code" | "claude code" => "claude".to_string(),
        "" => "openai".to_string(),
        other => {
            if other.starts_with("deepseek-") {
                "deepseek".to_string()
            } else if other.starts_with("claude-") {
                "claude".to_string()
            } else if other.starts_with("gpt-")
                || other.starts_with("o1")
                || other.starts_with("o3")
                || other.starts_with("o4")
            {
                "openai".to_string()
            } else {
                other.to_string()
            }
        }
    }
}

pub fn normalize_execution_speed(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sprint" | "fast" => "sprint".to_string(),
        "deep" | "thorough" => "deep".to_string(),
        _ => "deep".to_string(),
    }
}

fn normalize_auto_review_mode(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "disable" | "none" => "off".to_string(),
        "warn" | "warning" => "warn".to_string(),
        "block" | "strict" | "enforce" => "block".to_string(),
        _ => "off".to_string(),
    }
}

fn normalize_auto_review_severity_threshold(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" | "crit" | "p0" | "c4" => "critical".to_string(),
        "high" | "p1" | "h3" => "high".to_string(),
        "medium" | "med" | "p2" | "m2" => "medium".to_string(),
        "low" | "p3" | "l1" => "low".to_string(),
        _ => "high".to_string(),
    }
}

pub fn resolve_model_alias(model: &str) -> String {
    match model.trim().to_ascii_lowercase().as_str() {
        "opus" => "claude-opus-4-1".to_string(),
        "sonnet" => "claude-3-7-sonnet-latest".to_string(),
        "haiku" => "claude-3-5-haiku-latest".to_string(),
        other => other.to_string(),
    }
}

pub fn default_model(provider: &str) -> &'static str {
    match normalize_provider_name(provider).as_str() {
        "claude" => "sonnet",
        "deepseek" => "deepseek-v4-pro",
        _ => "gpt-4.1-mini",
    }
}

pub fn is_model_compatible_with_provider(provider: &str, model: &str) -> bool {
    let normalized_model = resolve_model_alias(model);
    if normalized_model.trim().is_empty() {
        return false;
    }
    let normalized_model = normalized_model.to_ascii_lowercase();
    match normalize_provider_name(provider).as_str() {
        "deepseek" => normalized_model.starts_with("deepseek-"),
        "claude" => normalized_model.starts_with("claude-"),
        _ => !normalized_model.starts_with("deepseek-") && !normalized_model.starts_with("claude-"),
    }
}

pub fn reconcile_model_for_provider(provider: &str, model: &str) -> (String, bool) {
    let normalized_provider = normalize_provider_name(provider);
    let normalized_model = resolve_model_alias(model);
    if is_model_compatible_with_provider(&normalized_provider, &normalized_model) {
        (normalized_model, false)
    } else {
        (
            resolve_model_alias(default_model(&normalized_provider)),
            true,
        )
    }
}

pub fn api_env_status() -> Vec<(String, String)> {
    vec![
        (
            "OPENAI_API_KEY".to_string(),
            if env::var("OPENAI_API_KEY").is_ok() {
                "set"
            } else {
                "missing"
            }
            .to_string(),
        ),
        (
            "OPENAI_BASE_URL".to_string(),
            env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        ),
        (
            "DEEPSEEK_API_KEY".to_string(),
            if env::var("DEEPSEEK_API_KEY").is_ok() {
                "set"
            } else {
                "missing"
            }
            .to_string(),
        ),
        (
            "DEEPSEEK_BASE_URL".to_string(),
            env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com/v1".to_string()),
        ),
        (
            "ANTHROPIC_API_KEY".to_string(),
            if env::var("ANTHROPIC_API_KEY").is_ok() {
                "set"
            } else {
                "missing"
            }
            .to_string(),
        ),
        (
            "ANTHROPIC_BASE_URL".to_string(),
            env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
        ),
    ]
}

pub fn apply_api_key_env(cfg: &AppConfig) {
    let Some(provider) = cfg.api_key_provider.as_deref() else {
        return;
    };
    let Some(key) = cfg.api_key.as_deref() else {
        return;
    };

    match provider {
        "openai" => env::set_var("OPENAI_API_KEY", key),
        "deepseek" => env::set_var("DEEPSEEK_API_KEY", key),
        "claude" => env::set_var("ANTHROPIC_API_KEY", key),
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    use super::{
        is_model_compatible_with_provider, normalize_execution_speed, normalize_provider_name,
        reconcile_model_for_provider, AppConfig,
    };

    #[test]
    fn normalize_provider_aliases() {
        assert_eq!(normalize_provider_name("Claude Code"), "claude");
        assert_eq!(normalize_provider_name("deepseek-ai"), "deepseek");
        assert_eq!(normalize_provider_name("deepseek-reasoner"), "deepseek");
        assert_eq!(
            normalize_provider_name("claude-3-7-sonnet-latest"),
            "claude"
        );
        assert_eq!(normalize_provider_name("gpt-4.1-mini"), "openai");
        assert_eq!(normalize_provider_name(""), "openai");
    }

    #[test]
    fn model_compatibility_rules() {
        assert!(is_model_compatible_with_provider(
            "deepseek",
            "deepseek-v4-pro"
        ));
        assert!(!is_model_compatible_with_provider(
            "deepseek",
            "gpt-4.1-mini"
        ));
        assert!(is_model_compatible_with_provider("claude", "sonnet"));
        assert!(!is_model_compatible_with_provider(
            "openai",
            "claude-3-7-sonnet-latest"
        ));
    }

    #[test]
    fn reconcile_model_fallbacks() {
        let (model, fallback) = reconcile_model_for_provider("deepseek", "gpt-4.1-mini");
        assert!(fallback);
        assert_eq!(model, "deepseek-v4-pro");
    }

    #[test]
    fn normalize_execution_speed_aliases() {
        assert_eq!(normalize_execution_speed("sprint"), "sprint");
        assert_eq!(normalize_execution_speed("fast"), "sprint");
        assert_eq!(normalize_execution_speed("deep"), "deep");
        assert_eq!(normalize_execution_speed("thorough"), "deep");
        assert_eq!(normalize_execution_speed("unknown"), "deep");
    }

    #[test]
    fn app_config_default_uses_deep_speed() {
        assert_eq!(AppConfig::default().execution_speed, "deep");
    }

    #[test]
    fn app_config_default_uses_safe_permission_mode() {
        assert_eq!(AppConfig::default().permission_mode, "on-request");
    }

    #[test]
    fn app_config_default_auto_review_is_safe() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.auto_review_mode, "off");
        assert_eq!(cfg.auto_review_severity_threshold, "high");
    }
}
