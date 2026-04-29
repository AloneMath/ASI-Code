use crate::config::AppConfig;
use crate::runtime::Runtime;
use crate::session::SessionStore;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct GatewayState {
    cfg: AppConfig,
    store: SessionStore,
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateSessionResponse {
    session_id: String,
    provider: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct SessionSummary {
    session_id: String,
    provider: String,
    model: String,
    message_count: usize,
    last_stop_reason_raw: Option<String>,
    last_stop_reason_alias: Option<String>,
}

#[derive(Debug, Serialize)]
struct SessionListResponse {
    sessions: Vec<SessionSummary>,
}

#[derive(Debug, Deserialize)]
struct TurnRequest {
    input: String,
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
}

#[derive(Debug, Serialize)]
struct TurnResponse {
    message: String,
    stop_reason: String,
    stop_reason_alias: String,
    runtime_stop_reason_last_raw: String,
    runtime_stop_reason_last_alias: String,
    session_id: String,
    provider: String,
    model: String,
    turn_cost_usd: f64,
    total_cost_usd: f64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.into(),
        }),
    )
}

fn new_runtime_from_cfg(cfg: &AppConfig) -> Runtime {
    let mut rt = Runtime::new(
        cfg.provider.clone(),
        cfg.model.clone(),
        cfg.permission_mode.clone(),
        cfg.max_turns,
    );
    rt.extended_thinking = cfg.extended_thinking;
    crate::apply_runtime_flags_from_cfg(&mut rt, cfg);
    rt
}

pub(crate) fn serve(
    listen: String,
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    project: Option<String>,
) -> Result<(), String> {
    if let Some(path) = project.as_deref() {
        let _ = crate::set_project_dir(path)?;
    }

    let cfg = crate::resolve_cfg(provider, model, permission_mode);
    let store = SessionStore::default()?;
    let state = GatewayState { cfg, store };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/sessions", post(create_session).get(list_sessions))
        .route("/v1/sessions/:id", get(get_session))
        .route("/v1/sessions/:id/turns", post(run_turn))
        .with_state(Arc::new(Mutex::new(state)));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|e| e.to_string())?;
        println!("gateway listening on http://{}", listen);
        axum::serve(listener, app).await.map_err(|e| e.to_string())
    })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn create_session(
    State(state): State<Arc<Mutex<GatewayState>>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let guard = state
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "state lock poisoned"))?;
    let resolved = crate::resolve_cfg(req.provider, req.model, req.permission_mode);

    // Persist an initial session with the runtime system prompt so the id can be used immediately.
    let rt = new_runtime_from_cfg(&resolved);
    let saved = guard
        .store
        .save(&resolved.provider, &resolved.model, rt.as_json_messages())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let session_id = saved
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "invalid session path"))?
        .to_string();

    // Keep compiler happy with state.cfg being used in future extensions.
    let _ = &guard.cfg;

    Ok(Json(CreateSessionResponse {
        session_id,
        provider: resolved.provider,
        model: resolved.model,
    }))
}

async fn list_sessions(
    State(state): State<Arc<Mutex<GatewayState>>>,
) -> Result<Json<SessionListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let guard = state
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "state lock poisoned"))?;

    let ids = guard
        .store
        .list_sessions(200)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let mut sessions = Vec::new();
    for sid in ids {
        if let Ok(sess) = guard.store.load(&sid) {
            sessions.push(SessionSummary {
                session_id: sid,
                provider: sess.provider,
                model: sess.model,
                message_count: sess.messages.len(),
                last_stop_reason_raw: sess.last_stop_reason_raw,
                last_stop_reason_alias: sess.last_stop_reason_alias,
            });
        }
    }

    Ok(Json(SessionListResponse { sessions }))
}

async fn get_session(
    State(state): State<Arc<Mutex<GatewayState>>>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionSummary>, (StatusCode, Json<ErrorResponse>)> {
    let guard = state
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "state lock poisoned"))?;

    let sess = guard
        .store
        .load(&session_id)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("session not found: {}", e)))?;
    Ok(Json(SessionSummary {
        session_id,
        provider: sess.provider,
        model: sess.model,
        message_count: sess.messages.len(),
        last_stop_reason_raw: sess.last_stop_reason_raw,
        last_stop_reason_alias: sess.last_stop_reason_alias,
    }))
}

async fn run_turn(
    State(state): State<Arc<Mutex<GatewayState>>>,
    Path(session_id): Path<String>,
    Json(req): Json<TurnRequest>,
) -> Result<Json<TurnResponse>, (StatusCode, Json<ErrorResponse>)> {
    if req.input.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "input is empty"));
    }

    let guard = state
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "state lock poisoned"))?;

    let stored = guard
        .store
        .load(&session_id)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("session not found: {}", e)))?;

    let mut cfg = guard.cfg.clone();
    if let Some(p) = req.provider {
        cfg.provider = crate::config::normalize_provider_name(&p);
    } else {
        cfg.provider = crate::config::normalize_provider_name(&stored.provider);
    }
    if let Some(m) = req.model {
        cfg.model = crate::config::resolve_model_alias(&m);
    } else {
        cfg.model = crate::config::resolve_model_alias(&stored.model);
    }
    if let Some(pm) = req.permission_mode {
        cfg.permission_mode = pm;
    }

    let (reconciled_model, fallback) =
        crate::config::reconcile_model_for_provider(&cfg.provider, &cfg.model);
    if fallback {
        eprintln!(
            "WARN model {} incompatible with provider {}, fallback={}",
            cfg.model, cfg.provider, reconciled_model
        );
    }
    cfg.model = reconciled_model;
    crate::config::apply_api_key_env(&cfg);

    let mut rt = new_runtime_from_cfg(&cfg);
    rt.load_session_messages(cfg.provider.clone(), cfg.model.clone(), stored.messages);
    let result = rt.run_turn(&req.input);

    guard
        .store
        .save_with_id_and_meta_and_stop_reason(
            &session_id,
            &cfg.provider,
            &cfg.model,
            rt.as_json_messages(),
            stored.meta.clone(),
            Some(rt.last_stop_reason_raw().to_string()),
            Some(rt.last_stop_reason_alias().to_string()),
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(TurnResponse {
        message: result.text,
        stop_reason_alias: Runtime::stop_reason_alias(&result.stop_reason).to_string(),
        runtime_stop_reason_last_raw: rt.last_stop_reason_raw().to_string(),
        runtime_stop_reason_last_alias: rt.last_stop_reason_alias().to_string(),
        stop_reason: result.stop_reason,
        session_id,
        provider: cfg.provider,
        model: cfg.model,
        turn_cost_usd: result.turn_cost_usd,
        total_cost_usd: result.total_cost_usd,
    }))
}

#[cfg(test)]
mod tests {
    use super::{SessionSummary, TurnResponse};

    #[test]
    fn turn_response_serializes_stop_reason_alias_fields() {
        let payload = TurnResponse {
            message: "ok".to_string(),
            stop_reason: "stop".to_string(),
            stop_reason_alias: "completed".to_string(),
            runtime_stop_reason_last_raw: "stop".to_string(),
            runtime_stop_reason_last_alias: "completed".to_string(),
            session_id: "sid".to_string(),
            provider: "openai".to_string(),
            model: "gpt-5.3-codex".to_string(),
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
        };

        let v = serde_json::to_value(&payload).expect("serialize gateway turn response");
        assert_eq!(v.get("stop_reason").and_then(|x| x.as_str()), Some("stop"));
        assert_eq!(
            v.get("stop_reason_alias").and_then(|x| x.as_str()),
            Some("completed")
        );
        assert_eq!(
            v.get("runtime_stop_reason_last_raw")
                .and_then(|x| x.as_str()),
            Some("stop")
        );
        assert_eq!(
            v.get("runtime_stop_reason_last_alias")
                .and_then(|x| x.as_str()),
            Some("completed")
        );
    }

    #[test]
    fn session_summary_serializes_last_stop_reason_fields() {
        let payload = SessionSummary {
            session_id: "sid".to_string(),
            provider: "openai".to_string(),
            model: "gpt-5.3-codex".to_string(),
            message_count: 3,
            last_stop_reason_raw: Some("stop".to_string()),
            last_stop_reason_alias: Some("completed".to_string()),
        };

        let v = serde_json::to_value(&payload).expect("serialize session summary");
        assert_eq!(
            v.get("last_stop_reason_raw").and_then(|x| x.as_str()),
            Some("stop")
        );
        assert_eq!(
            v.get("last_stop_reason_alias").and_then(|x| x.as_str()),
            Some("completed")
        );
    }
}
