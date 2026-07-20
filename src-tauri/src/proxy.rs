use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{oneshot, Mutex},
};
use uuid::Uuid;

use crate::{
    catalog::{RouteTarget, SharedCatalog, SharedRoutes},
    content_encoding::{decode_request_body, get_content_encoding},
    credentials::SecretMaterial,
    domain::ProxyRequestEvent,
    providers,
    scheduler::{ScheduleState, SelectionLayer},
    storage::{Lease, Storage, UsageDelta},
    vault::SecretVault,
};

#[derive(Clone)]
struct ProxyState {
    catalog: SharedCatalog,
    secret: Arc<String>,
    routes: SharedRoutes,
    storage: Arc<Storage>,
    vault: Arc<SecretVault>,
    metrics: Arc<UsageMetrics>,
    client: reqwest::Client,
}

pub struct UsageMetrics {
    request_count: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    cache_hits: AtomicU64,
    cache_observations: AtomicU64,
}

impl UsageMetrics {
    pub fn new() -> Self {
        Self {
            request_count: AtomicU64::new(0),
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_observations: AtomicU64::new(0),
        }
    }

    fn record_request(&self, body: &Value) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        let estimated = serde_json::to_string(body)
            .map(|text| (text.len() as u64 / 4).max(1))
            .unwrap_or(0);
        self.input_tokens.fetch_add(estimated, Ordering::Relaxed);
    }

    fn record_response(&self, body: &Value) {
        let output = body
            .pointer("/usage/output_tokens")
            .or_else(|| body.pointer("/usage/completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                serde_json::to_string(body)
                    .map(|text| text.len() as u64 / 4)
                    .unwrap_or(0)
            });
        self.output_tokens.fetch_add(output, Ordering::Relaxed);
        if let Some(cached) = body
            .pointer("/usage/input_tokens_details/cached_tokens")
            .or_else(|| body.pointer("/usage/prompt_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
        {
            self.cache_observations.fetch_add(1, Ordering::Relaxed);
            if cached > 0 {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub struct ProxyRuntime {
    pub port: u16,
    pub secret: Arc<String>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ProxyRuntime {
    pub async fn stop(&self) {
        if let Some(sender) = self.shutdown.lock().await.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.lock().await.take() {
            if let Err(error) = task.await {
                tracing::warn!(%error, "proxy server task did not stop cleanly");
            }
        }
    }
}

fn authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {secret}").as_str())
}

async fn health(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.secret) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid local proxy token",
        );
    }
    let catalog = state.catalog.read().await;
    Json(json!({
        "ok": true,
        "catalogRevision": catalog.models.len(),
        "instance": "codex-spur"
    }))
    .into_response()
}

async fn models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.secret) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid local proxy token",
        );
    }
    let catalog = state.catalog.read().await.clone();
    if catalog.models.is_empty() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "model_catalog_empty",
            "No selected models have been published yet.",
        );
    }
    Json(catalog).into_response()
}

fn estimated_tokens(value: &Value) -> i64 {
    serde_json::to_string(value)
        .map(|text| (text.len() as i64 / 4).max(1))
        .unwrap_or(0)
}

fn response_usage(value: &Value) -> (i64, i64, i64) {
    let output = value
        .pointer("/usage/output_tokens")
        .or_else(|| value.pointer("/usage/completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| estimated_tokens(value));
    let cached = value
        .pointer("/usage/input_tokens_details/cached_tokens")
        .or_else(|| value.pointer("/usage/prompt_tokens_details/cached_tokens"))
        .and_then(Value::as_i64);
    (
        output,
        i64::from(cached.is_some()),
        i64::from(cached.unwrap_or(0) > 0),
    )
}

async fn responses(
    State(state): State<ProxyState>,
    mut headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &state.secret) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid local proxy token",
        );
    }
    // ChatGPT Desktop (logged-in) often sends zstd-compressed JSON bodies.
    // Parse only after Content-Encoding has been applied.
    let body = match decode_request_body(&mut headers, body) {
        Ok(body) => body,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid_encoding", &message);
        }
    };
    let mut parsed = match parse_json_object_body(&body) {
        Ok(value) => value,
        Err(message) => {
            let hint = get_content_encoding(&headers)
                .map(|encoding| format!(" content-encoding={encoding}"))
                .unwrap_or_default();
            let first = body
                .iter()
                .take(8)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("{message}{hint}; first_bytes=[{first}]"),
            );
        }
    };
    let affinity = affinity_inputs(&headers, &parsed);
    let model = parsed
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let target = state.routes.read().await.get(&model).cloned();
    let Some(target) = target else {
        return error_response(
            StatusCode::NOT_FOUND,
            "unknown_route",
            &format!("No Codex Spur route is published for model `{model}`."),
        );
    };
    if target.base_url.is_empty() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_not_configured",
            "The selected provider does not have a Base URL.",
        );
    }
    state.metrics.record_request(&parsed);
    let _ = state
        .storage
        .record_usage(
            &target.provider_id,
            &target.upstream_model,
            &UsageDelta {
                request_count: 1,
                input_tokens: estimated_tokens(&parsed),
                output_tokens: 0,
                cache_observations: 0,
                cache_hits: 0,
                failed_requests: 0,
            },
        )
        .await;
    if let Some(object) = parsed.as_object_mut() {
        object.insert("model".into(), Value::String(target.upstream_model.clone()));
    }
    if target.protocol.to_ascii_lowercase().contains("chat") {
        // Chat Completions conversion maps reasoning itself; do not pre-mutate
        // reasoning.effort into provider-internal tokens like "disabled"/"enabled".
        forward_chat_compatible(&state, &target, parsed, &affinity).await
    } else {
        map_reasoning(&target, &mut parsed);
        forward_responses_compatible(&state, &target, parsed, &affinity).await
    }
}

async fn forward_responses_compatible(
    state: &ProxyState,
    target: &RouteTarget,
    request_body: Value,
    affinity: &AffinityInputs,
) -> Response {
    let endpoint = endpoint(&target.base_url, &target.kind, "responses");
    let max_switches = state
        .storage
        .max_failover_switches(&target.provider_id)
        .await
        .unwrap_or(3)
        .max(1) as usize;
    let mut exclude: Vec<String> = Vec::new();
    let started = std::time::Instant::now();
    for attempt in 0..max_switches {
        let mut request = state.client.post(&endpoint).json(&request_body);
        let auth = match upstream_auth(state, &target.provider_id, affinity, &exclude).await {
            Ok(Some(auth)) => {
                request = apply_upstream_headers(request, &auth, target);
                Some(auth)
            }
            Ok(None) => None,
            Err(response) => return response,
        };
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(auth) = &auth {
                    record_diag(
                        state,
                        target,
                        auth,
                        attempt,
                        "transport",
                        false,
                        Some("upstream transport error"),
                        started.elapsed().as_millis() as i64,
                    )
                    .await;
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                }
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                );
            }
        };
        let status = response.status();
        if is_failover_status(status) && attempt + 1 < max_switches {
            if let Some(auth) = auth {
                let category = status_category(status);
                let cooldown = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                handle_upstream_failure(
                    state,
                    &target.provider_id,
                    &auth,
                    status,
                    response.headers(),
                )
                .await;
                record_diag(
                    state,
                    target,
                    &auth,
                    attempt,
                    category,
                    cooldown,
                    Some(category),
                    started.elapsed().as_millis() as i64,
                )
                .await;
                exclude.push(auth.credential_id);
                let _ = state.storage.release_lease(&auth.lease_id).await;
                continue;
            }
        }
        if let Some(auth) = &auth {
            let category = if status.is_success() {
                "ok"
            } else {
                status_category(status)
            };
            record_diag(
                state,
                target,
                auth,
                attempt,
                category,
                false,
                if status.is_success() {
                    None
                } else {
                    Some(category)
                },
                started.elapsed().as_millis() as i64,
            )
            .await;
            let _ = state.storage.release_lease(&auth.lease_id).await;
        }
        return passthrough(
            response,
            &state.metrics,
            &state.storage,
            &target.provider_id,
            &target.upstream_model,
        )
        .await;
    }
    error_response(
        StatusCode::BAD_GATEWAY,
        "upstream_retry_exhausted",
        "All eligible accounts failed",
    )
}

async fn forward_chat_compatible(
    state: &ProxyState,
    target: &RouteTarget,
    request_body: Value,
    affinity: &AffinityInputs,
) -> Response {
    let wants_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Codex Desktop talks Responses API; DeepSeek/Kimi expose Chat Completions only.
    // Naive passthrough of Responses `tools` (type/name/parameters, local_shell, …)
    // makes upstream reject with 400. Convert like Nice Switch's transform_codex_chat.
    let chat_body = responses_to_chat_completions(&request_body, &target.upstream_model);
    let endpoint = endpoint(&target.base_url, &target.kind, "chat/completions");
    let max_switches = state
        .storage
        .max_failover_switches(&target.provider_id)
        .await
        .unwrap_or(3)
        .max(1) as usize;
    let mut exclude: Vec<String> = Vec::new();
    let started = std::time::Instant::now();
    for attempt in 0..max_switches {
        let mut request = state.client.post(&endpoint).json(&chat_body);
        let auth = match upstream_auth(state, &target.provider_id, affinity, &exclude).await {
            Ok(Some(auth)) => {
                request = apply_upstream_headers(request, &auth, target);
                Some(auth)
            }
            Ok(None) => None,
            Err(response) => return response,
        };
        let upstream = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(auth) = &auth {
                    record_diag(
                        state,
                        target,
                        auth,
                        attempt,
                        "transport",
                        false,
                        Some("upstream transport error"),
                        started.elapsed().as_millis() as i64,
                    )
                    .await;
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                }
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                );
            }
        };
        let status = upstream.status();
        if is_failover_status(status) && attempt + 1 < max_switches {
            if let Some(auth) = auth {
                let category = status_category(status);
                let cooldown = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                handle_upstream_failure(
                    state,
                    &target.provider_id,
                    &auth,
                    status,
                    upstream.headers(),
                )
                .await;
                record_diag(
                    state,
                    target,
                    &auth,
                    attempt,
                    category,
                    cooldown,
                    Some(category),
                    started.elapsed().as_millis() as i64,
                )
                .await;
                exclude.push(auth.credential_id);
                let _ = state.storage.release_lease(&auth.lease_id).await;
                continue;
            }
        }
        if let Some(auth) = &auth {
            let category = if status.is_success() {
                "ok"
            } else {
                status_category(status)
            };
            record_diag(
                state,
                target,
                auth,
                attempt,
                category,
                false,
                if status.is_success() {
                    None
                } else {
                    Some(category)
                },
                started.elapsed().as_millis() as i64,
            )
            .await;
            let _ = state.storage.release_lease(&auth.lease_id).await;
        }
        if wants_stream {
            return adapt_chat_stream(
                upstream,
                &state.metrics,
                &state.storage,
                &target.provider_id,
                &target.upstream_model,
            )
            .await;
        }
        let payload = match upstream.json::<Value>().await {
            Ok(payload) => payload,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "invalid_upstream_json",
                    &format!("Chat Completions response was not valid JSON: {error}"),
                )
            }
        };
        if !status.is_success() {
            return (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(payload),
            )
                .into_response();
        }
        state.metrics.record_response(&payload);
        let (output_tokens, cache_observations, cache_hits) = response_usage(&payload);
        let _ = state
            .storage
            .record_usage(
                &target.provider_id,
                &target.upstream_model,
                &UsageDelta {
                    request_count: 0,
                    input_tokens: 0,
                    output_tokens,
                    cache_observations,
                    cache_hits,
                    failed_requests: i64::from(!status.is_success()),
                },
            )
            .await;
        let text = payload
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("resp_codex_select");
        let usage = chat_usage_to_responses_usage(payload.get("usage"));
        return Json(json!({
            "id": id,
            "object": "response",
            "status": "completed",
            "model": target.upstream_model,
            "output": [{
                "id": format!("msg_{id}"),
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text, "annotations": []}]
            }],
            "usage": usage
        }))
        .into_response();
    }
    error_response(
        StatusCode::BAD_GATEWAY,
        "upstream_retry_exhausted",
        "All eligible accounts failed",
    )
}

async fn adapt_chat_stream(
    upstream: reqwest::Response,
    metrics: &UsageMetrics,
    storage: &Storage,
    provider_id: &str,
    model_id: &str,
) -> Response {
    let status = upstream.status();
    // reqwest is built without auto-decompress; honor Content-Encoding ourselves.
    let encoding = get_content_encoding(upstream.headers());
    let raw = match upstream.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_body_error",
                &error.to_string(),
            )
        }
    };
    let body_bytes = match encoding.as_deref() {
        Some(encoding) => match crate::content_encoding::decompress_body(encoding, &raw) {
            Ok(Some(decoded)) => Bytes::from(decoded),
            Ok(None) | Err(_) => raw,
        },
        None => raw,
    };
    let body = match String::from_utf8(body_bytes.to_vec()) {
        Ok(text) => text,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_body_error",
                &format!("Upstream body was not valid UTF-8: {error}"),
            )
        }
    };
    if !status.is_success() {
        let payload = serde_json::from_str::<Value>(&body)
            .unwrap_or_else(|_| json!({"error": {"message": "上游流式请求失败"}}));
        let _ = storage
            .record_usage(
                provider_id,
                model_id,
                &UsageDelta {
                    request_count: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_observations: 0,
                    cache_hits: 0,
                    failed_requests: 1,
                },
            )
            .await;
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(payload),
        )
            .into_response();
    }
    let mut text = String::new();
    let mut raw_usage: Option<Value> = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(payload) = serde_json::from_str::<Value>(data) {
            if let Some(delta) = payload
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                text.push_str(delta);
            }
            if let Some(next_usage) = payload.get("usage").filter(|value| !value.is_null()) {
                raw_usage = Some(next_usage.clone());
            }
        }
    }
    // Codex Desktop deserializes ResponseCompleted.usage and requires input_tokens
    // (Responses shape). Chat Completions only has prompt_tokens — map like CC Switch.
    let usage = chat_usage_to_responses_usage(raw_usage.as_ref());
    let response_id = format!("resp_{}", Uuid::new_v4());
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "object": "response",
            "status": "completed",
            "model": model_id,
            "output": [{"id": format!("msg_{response_id}"), "type": "message", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": text, "annotations": []}]}],
            "usage": usage
        }
    });
    metrics.record_response(&json!({"usage": usage}));
    let (output_tokens, cache_observations, cache_hits) = response_usage(&json!({"usage": usage}));
    let _ = storage
        .record_usage(
            provider_id,
            model_id,
            &UsageDelta {
                request_count: 0,
                input_tokens: 0,
                output_tokens,
                cache_observations,
                cache_hits,
                failed_requests: 0,
            },
        )
        .await;
    let delta = json!({"type": "response.output_text.delta", "item_id": format!("msg_{response_id}"), "output_index": 0, "content_index": 0, "delta": text});
    let stream = format!("event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\ndata: [DONE]\n\n", serde_json::to_string(&delta).unwrap_or_default(), serde_json::to_string(&completed).unwrap_or_default());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(stream))
        .unwrap_or_else(|_| {
            error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_response_error",
                "Failed to build streaming response",
            )
        })
}

struct AffinityInputs {
    previous_response_id: Option<String>,
    session_key: Option<String>,
}

struct UpstreamAuth {
    credential_id: String,
    lease_id: String,
    token: String,
    account_id: Option<String>,
    layer: SelectionLayer,
    sticky_escaped: bool,
}

fn is_failover_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn apply_upstream_headers(
    mut request: reqwest::RequestBuilder,
    auth: &UpstreamAuth,
    target: &RouteTarget,
) -> reqwest::RequestBuilder {
    request = request.bearer_auth(&auth.token);
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    if target.kind == "openai" && target.base_url.contains("chatgpt.com") {
        request = request
            .header("originator", providers::CODEX_ORIGINATOR)
            .header("version", providers::CODEX_CLIENT_VERSION);
    }
    if target.kind == "kimi" {
        request = request.header(
            reqwest::header::USER_AGENT,
            "claude-cli/1.0.0 (Codex Spur)",
        );
    }
    request
}

async fn handle_upstream_failure(
    state: &ProxyState,
    provider_id: &str,
    auth: &UpstreamAuth,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
) {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = state
            .storage
            .mark_schedule_state(
                &auth.credential_id,
                ScheduleState::AuthInvalid,
                false,
                Some("上游认证失败 (401)"),
                None,
            )
            .await;
        return;
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        let _ = state
            .storage
            .mark_schedule_state(
                &auth.credential_id,
                ScheduleState::Entitlement,
                false,
                Some("上游权限/权益失败 (403)"),
                None,
            )
            .await;
        return;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok());
        let default_secs = state
            .storage
            .default_429_cooldown_secs(provider_id)
            .await
            .unwrap_or(30);
        let _ = state
            .storage
            .apply_rate_limit_cooldown(&auth.credential_id, default_secs, retry_after)
            .await;
    }
}

async fn upstream_auth(
    state: &ProxyState,
    provider_id: &str,
    affinity: &AffinityInputs,
    exclude: &[String],
) -> Result<Option<UpstreamAuth>, Response> {
    let lease = state
        .storage
        .select_for_request(
            provider_id,
            affinity.previous_response_id.as_deref(),
            affinity.session_key.as_deref(),
            exclude,
        )
        .await
        .map_err(|error| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "account_scheduler_error",
                &error.to_string(),
            )
        })?;

    let Some(lease) = lease else {
        // Last resort for providers without pool members yet.
        let credential = state
            .storage
            .first_healthy_credential(provider_id)
            .await
            .map_err(|error| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential_store_error",
                    &error.to_string(),
                )
            })?;
        let Some(credential) = credential else {
            return Ok(None);
        };
        return decrypt_auth(
            state,
            &Lease {
                id: format!("ephemeral-{}", Uuid::new_v4()),
                credential_id: credential.id.clone(),
                layer: crate::scheduler::SelectionLayer::LoadBalance,
                sticky_escaped: false,
            },
            credential,
        );
    };

    let credential = state
        .storage
        .get_credential(&lease.credential_id)
        .await
        .map_err(|error| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "credential_store_error",
                &error.to_string(),
            )
        })?;
    let Some(credential) = credential else {
        let _ = state.storage.release_lease(&lease.id).await;
        return Ok(None);
    };
    decrypt_auth(state, &lease, credential)
}

#[allow(clippy::result_large_err)]
fn decrypt_auth(
    state: &ProxyState,
    lease: &Lease,
    credential: crate::storage::StoredCredential,
) -> Result<Option<UpstreamAuth>, Response> {
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "credential_decrypt_error",
                &error.to_string(),
            )
        })?;
    let secret = SecretMaterial::from_json_bytes(plaintext.as_slice()).map_err(|error| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential_decode_error",
            &error.to_string(),
        )
    })?;
    let token = secret
        .api_key
        .or(secret.access_token)
        .or(secret.session_token);
    Ok(token.map(|token| UpstreamAuth {
        credential_id: credential.id,
        lease_id: lease.id.clone(),
        token,
        account_id: credential.account_id,
        layer: lease.layer,
        sticky_escaped: lease.sticky_escaped,
    }))
}

fn status_category(status: reqwest::StatusCode) -> &'static str {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        "auth_invalid"
    } else if status == reqwest::StatusCode::FORBIDDEN {
        "entitlement"
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        "rate_limited"
    } else if status.is_client_error() {
        "upstream_4xx"
    } else if status.is_server_error() {
        "upstream_5xx"
    } else {
        "ok"
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_diag(
    state: &ProxyState,
    target: &RouteTarget,
    auth: &UpstreamAuth,
    attempt: usize,
    result_category: &str,
    cooldown_applied: bool,
    error_summary: Option<&str>,
    latency_ms_total: i64,
) {
    let fingerprint = state
        .storage
        .credential_fingerprint_prefix(&auth.credential_id)
        .await
        .ok()
        .flatten();
    let max_events = state
        .storage
        .diagnostics_max_events()
        .await
        .unwrap_or(200);
    let event = ProxyRequestEvent {
        id: Uuid::new_v4().to_string(),
        created_at: String::new(),
        route_slug: Some(target.upstream_model.clone()),
        display_name: None,
        provider_id: Some(target.provider_id.clone()),
        upstream_model: Some(target.upstream_model.clone()),
        protocol: Some(target.protocol.clone()),
        selection_layer: auth.layer.as_str().to_string(),
        sticky_escaped: auth.sticky_escaped,
        account_fingerprint: fingerprint,
        schedule_state: None,
        result_category: result_category.to_string(),
        failover_attempt: attempt as u32,
        latency_ms_total: Some(latency_ms_total),
        first_token_ms: None,
        cooldown_applied,
        error_summary: error_summary.map(str::to_string),
    };
    let _ = state
        .storage
        .record_proxy_request_event(&event, max_events)
        .await;
}

/// Layered affinity inputs: previous_response_id and session are separate.
fn affinity_inputs(headers: &HeaderMap, request: &Value) -> AffinityInputs {
    let previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let session_raw = headers
        .get("x-codex-session-id")
        .or_else(|| headers.get("x-session-id"))
        .or_else(|| headers.get("session_id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            request
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            request
                .pointer("/metadata/session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    let session_key = session_raw.map(|raw| {
        let mut hasher = Sha256::new();
        hasher.update(b"codex-select-session-v1\0");
        hasher.update(raw.as_bytes());
        hex::encode(hasher.finalize())
    });

    AffinityInputs {
        previous_response_id,
        session_key,
    }
}

fn map_reasoning(target: &RouteTarget, request: &mut Value) {
    let Some(codex_effort) = request.pointer("/reasoning/effort").and_then(Value::as_str) else {
        return;
    };
    // Use kind (openai/kimi/…), not instance provider_id (UUID).
    let profile = providers::reasoning_profile(&target.kind, &target.upstream_model);
    let upstream = profile
        .mappings
        .iter()
        .find(|mapping| mapping.codex_effort.as_str() == codex_effort)
        .map(|mapping| mapping.upstream_effort.clone());
    if let Some(upstream) = upstream {
        if let Some(reasoning) = request.get_mut("reasoning").and_then(Value::as_object_mut) {
            reasoning.insert("effort".into(), Value::String(upstream));
        }
    }
}

/// Parse Codex → proxy request bodies. Desktop occasionally double-encodes JSON as a
/// string, and structured-turn helpers may send empty bodies; reject with a useful
/// message instead of a bare "must be a JSON object".
fn parse_json_object_body(body: &Bytes) -> Result<Value, String> {
    if body.is_empty() {
        return Err("Request body is empty (expected a JSON object)".into());
    }
    let parsed = match serde_json::from_slice::<Value>(body) {
        Ok(value) => value,
        Err(error) => {
            return Err(format!(
                "Request body is not valid JSON ({} bytes): {error}",
                body.len()
            ));
        }
    };
    match parsed {
        Value::Object(_) => Ok(parsed),
        // Double-encoded: "\"{...}\"" → string whose content is a JSON object.
        Value::String(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok(Value::Object(map)),
            Ok(other) => Err(format!(
                "Request body string decoded to JSON {}, expected object",
                json_value_kind(&other)
            )),
            Err(error) => Err(format!(
                "Request body is a JSON string but not a nested object: {error}"
            )),
        },
        other => Err(format!(
            "Request body must be a JSON object (got {})",
            json_value_kind(&other)
        )),
    }
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Map Chat Completions `usage` onto Responses usage for Codex Desktop.
///
/// Desktop requires `input_tokens` on `response.completed`; Chat only exposes
/// `prompt_tokens` / `completion_tokens`. Always emit a full object (zeros when
/// missing) — never `null` — matching CC Switch's `chat_usage_to_responses_usage`.
fn chat_usage_to_responses_usage(usage: Option<&Value>) -> Value {
    let Some(usage) = usage.filter(|value| value.is_object() && !value.is_null()) else {
        return json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
            "output_tokens_details": { "reasoning_tokens": 0 }
        });
    };

    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);

    let mut result = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    let cached = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = usage
        .pointer("/prompt_tokens_details/cache_write_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cache_write_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    if cached > 0 || cache_write > 0 {
        result["input_tokens_details"] = json!({
            "cached_tokens": cached,
            "cache_write_tokens": cache_write
        });
    }

    if let Some(details) = usage
        .get("completion_tokens_details")
        .filter(|value| value.is_object())
    {
        let mut details = details.clone();
        if details.get("reasoning_tokens").is_none() {
            details["reasoning_tokens"] = json!(0);
        }
        result["output_tokens_details"] = details;
    } else {
        result["output_tokens_details"] = json!({ "reasoning_tokens": 0 });
    }

    if let Some(cache_read) = usage.get("cache_read_input_tokens") {
        result["cache_read_input_tokens"] = cache_read.clone();
    }
    if cache_write > 0 {
        result["cache_creation_input_tokens"] = json!(cache_write);
    }

    result
}

/// OpenAI-compat stream requests omit usage unless `stream_options.include_usage`.
fn inject_stream_include_usage(chat: &mut Value) {
    let is_stream = chat
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_stream {
        return;
    }
    match chat.get_mut("stream_options") {
        Some(Value::Object(opts)) => {
            opts.insert("include_usage".into(), json!(true));
        }
        _ => {
            chat.as_object_mut()
                .expect("chat object")
                .insert("stream_options".into(), json!({ "include_usage": true }));
        }
    }
}

/// Convert a Codex Responses request into OpenAI Chat Completions for Kimi/DeepSeek.
fn responses_to_chat_completions(request_body: &Value, upstream_model: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) = request_body.get("instructions") {
        let text = instruction_text(instructions);
        if !text.is_empty() {
            messages.push(json!({"role": "system", "content": text}));
        }
    }
    messages.extend(response_input_to_messages(request_body.get("input")));
    if messages.is_empty() {
        messages.push(json!({"role": "user", "content": ""}));
    }

    let wants_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut chat = json!({
        "model": upstream_model,
        "messages": messages,
        "stream": wants_stream,
    });
    inject_stream_include_usage(&mut chat);

    let tools = responses_tools_to_chat_tools(request_body.get("tools"));
    if !tools.is_empty() {
        chat.as_object_mut()
            .expect("chat object")
            .insert("tools".into(), Value::Array(tools));
        if let Some(tool_choice) = request_body.get("tool_choice") {
            chat.as_object_mut()
                .expect("chat object")
                .insert("tool_choice".into(), tool_choice.clone());
        }
    }

    // Optional Chat Completions knobs Codex may already set.
    for key in [
        "temperature",
        "top_p",
        "max_tokens",
        "max_completion_tokens",
        "stop",
        "user",
        "n",
        "presence_penalty",
        "frequency_penalty",
        "response_format",
        "seed",
    ] {
        if let Some(value) = request_body.get(key) {
            if !value.is_null() {
                chat.as_object_mut()
                    .expect("chat object")
                    .insert(key.into(), value.clone());
            }
        }
    }

    // Only inject OpenAI-style reasoning_effort when the Codex effort is a legal
    // Chat Completions enum. Never forward profile tokens like disabled/enabled/off.
    if let Some(effort) = request_body
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
    {
        if let Some(mapped) = chat_reasoning_effort(effort) {
            chat.as_object_mut()
                .expect("chat object")
                .insert("reasoning_effort".into(), Value::String(mapped.into()));
        }
    }

    chat
}

/// Map Codex ladder → Chat Completions `reasoning_effort` (DeepSeek/Kimi/OpenAI-compat).
/// Returns None to omit the field (e.g. none/minimal → no thinking param).
fn chat_reasoning_effort(codex_effort: &str) -> Option<&'static str> {
    match codex_effort {
        "none" | "minimal" | "disabled" | "off" => None,
        "low" => Some("low"),
        "medium" | "enabled" | "default" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" | "ultra" => Some("high"),
        _ => None,
    }
}

fn instruction_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.as_str().map(str::to_string).or_else(|| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Map Codex Responses roles onto Chat Completions roles accepted by
/// DeepSeek/Kimi (and OpenAI-compatible gateways).
///
/// Codex Desktop injects `developer` blocks (permissions, collaboration mode,
/// …). DeepSeek rejects them with:
/// `messages[N].role: unknown variant developer, expected one of system, user, …`.
fn responses_role_to_chat_role(role: &str) -> &'static str {
    match role {
        "system" | "developer" => "system",
        "assistant" => "assistant",
        "tool" => "tool",
        // latest_reminder is a DeepSeek-accepted role; treat as user for other
        // OpenAI-compatible upstreams that only allow the classic set.
        "user" | "latest_reminder" => "user",
        _ => "user",
    }
}

fn response_input_to_messages(input: Option<&Value>) -> Vec<Value> {
    match input {
        Some(Value::String(text)) => vec![json!({"role": "user", "content": text})],
        Some(Value::Array(items)) => {
            let mut messages = Vec::new();
            for item in items {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                // Skip tool/function call items for the first vertical slice; text only.
                if matches!(
                    item_type,
                    "function_call"
                        | "function_call_output"
                        | "custom_tool_call"
                        | "custom_tool_call_output"
                        | "reasoning"
                        | "web_search_call"
                ) {
                    continue;
                }
                let raw_role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
                let role = responses_role_to_chat_role(raw_role);
                if let Some(content) = item.get("content") {
                    let text = content_text(content);
                    if !text.is_empty() || role == "assistant" {
                        messages.push(json!({"role": role, "content": text}));
                    }
                    continue;
                }
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        messages.push(json!({"role": role, "content": text}));
                    }
                }
            }
            if messages.is_empty() {
                vec![json!({"role": "user", "content": ""})]
            } else {
                messages
            }
        }
        _ => vec![json!({"role": "user", "content": ""})],
    }
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("input_text").and_then(Value::as_str))
                    .or_else(|| part.get("output_text").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Map Codex Responses tools → Chat Completions `tools[].function` shape.
/// Drops unsupported kinds (local_shell, web_search, …) instead of forwarding raw rows.
fn responses_tools_to_chat_tools(tools: Option<&Value>) -> Vec<Value> {
    let Some(Value::Array(items)) = tools else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tool in items {
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
        // Already Chat Completions shaped.
        if tool.get("function").is_some() && (tool_type.is_empty() || tool_type == "function") {
            out.push(tool.clone());
            continue;
        }
        // Responses freeform function tool: {type:function, name, description, parameters}
        if tool_type == "function" || tool.get("name").is_some() {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let description = tool
                .get("description")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            let parameters = tool
                .get("parameters")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            out.push(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters
                }
            }));
            continue;
        }
        // local_shell / web_search / custom / namespace — not portable to DeepSeek/Kimi.
    }
    out
}

fn endpoint(base_url: &str, kind: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if kind == "openai" {
        return format!("{base}/{path}");
    }
    if base.ends_with("/v1") {
        format!("{base}/{path}")
    } else {
        format!("{base}/v1/{path}")
    }
}

async fn passthrough(
    response: reqwest::Response,
    metrics: &UsageMetrics,
    storage: &Storage,
    provider_id: &str,
    model_id: &str,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    match response.bytes().await {
        Ok(bytes) => {
            if let Ok(body) = serde_json::from_slice::<Value>(&bytes) {
                metrics.record_response(&body);
                let (output_tokens, cache_observations, cache_hits) = response_usage(&body);
                let _ = storage
                    .record_usage(
                        provider_id,
                        model_id,
                        &UsageDelta {
                            request_count: 0,
                            input_tokens: 0,
                            output_tokens,
                            cache_observations,
                            cache_hits,
                            failed_requests: i64::from(!status.is_success()),
                        },
                    )
                    .await;
            } else {
                let output_tokens = bytes.len() as i64 / 4;
                metrics
                    .output_tokens
                    .fetch_add(output_tokens as u64, Ordering::Relaxed);
                let _ = storage
                    .record_usage(
                        provider_id,
                        model_id,
                        &UsageDelta {
                            request_count: 0,
                            input_tokens: 0,
                            output_tokens,
                            cache_observations: 0,
                            cache_hits: 0,
                            failed_requests: i64::from(!status.is_success()),
                        },
                    )
                    .await;
            }
            let mut builder = Response::builder().status(status);
            if let Some(content_type) = content_type {
                builder = builder.header(header::CONTENT_TYPE, content_type);
            }
            builder.body(Body::from(bytes)).unwrap_or_else(|_| {
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "proxy_response_error",
                    "Failed to build proxy response",
                )
            })
        }
        Err(error) => error_response(
            StatusCode::BAD_GATEWAY,
            "upstream_body_error",
            &format!("Failed to read upstream response: {error}"),
        ),
    }
}

fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "type": kind,
                "message": message,
            }
        })),
    )
        .into_response()
}

/// Load or create a stable local proxy bearer so `~/.codex` stays valid across restarts.
pub fn load_or_create_secret(data_dir: &std::path::Path) -> anyhow::Result<String> {
    let path = data_dir.join("proxy_bearer_token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let secret = Uuid::new_v4().to_string();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&path, format!("{secret}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(secret)
}

pub async fn start(
    catalog: SharedCatalog,
    routes: SharedRoutes,
    storage: Arc<Storage>,
    vault: Arc<SecretVault>,
    preferred_port: u16,
) -> anyhow::Result<ProxyRuntime> {
    start_with_secret(
        catalog,
        routes,
        storage,
        vault,
        preferred_port,
        Uuid::new_v4().to_string(),
    )
    .await
}

pub async fn start_with_secret(
    catalog: SharedCatalog,
    routes: SharedRoutes,
    storage: Arc<Storage>,
    vault: Arc<SecretVault>,
    preferred_port: u16,
    secret: String,
) -> anyhow::Result<ProxyRuntime> {
    let secret = Arc::new(secret);
    let mut selected_port = preferred_port;
    let listener = loop {
        match TcpListener::bind(("127.0.0.1", selected_port)).await {
            Ok(listener) => break listener,
            Err(error) if selected_port < preferred_port + 32 => {
                tracing::warn!(port = selected_port, %error, "proxy port occupied, trying next port");
                selected_port += 1;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let metrics = Arc::new(UsageMetrics::new());
    let state = ProxyState {
        catalog,
        secret: Arc::clone(&secret),
        routes,
        storage,
        vault,
        metrics: Arc::clone(&metrics),
        client: reqwest::Client::builder()
            .user_agent("Codex-Spur/0.1")
            .build()?,
    };
    let router = Router::new()
        .route("/healthz", get(health))
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .with_state(state);
    let address: SocketAddr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        if let Err(error) = server.await {
            tracing::error!(%error, "proxy server stopped unexpectedly");
        }
    });
    Ok(ProxyRuntime {
        port: address.port(),
        secret,
        shutdown: Mutex::new(Some(shutdown_tx)),
        task: Mutex::new(Some(task)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn load_or_create_secret_is_stable_across_calls() {
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-proxy-secret-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let a = load_or_create_secret(&dir).expect("create");
        let b = load_or_create_secret(&dir).expect("reload");
        assert_eq!(a, b);
        assert!(!a.is_empty());
        let on_disk = std::fs::read_to_string(dir.join("proxy_bearer_token")).expect("file");
        assert_eq!(on_disk.trim(), a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn maps_response_inputs_to_chat_messages() {
        let messages = response_input_to_messages(Some(&json!([
            {"role": "user", "content": [{"type": "input_text", "text": "Hi"}]}
        ])));
        assert_eq!(messages[0]["content"], "Hi");
    }

    #[test]
    fn chat_usage_maps_prompt_tokens_to_input_tokens() {
        let mapped = chat_usage_to_responses_usage(Some(&json!({
            "prompt_tokens": 4,
            "completion_tokens": 2,
            "total_tokens": 6,
            "completion_tokens_details": { "reasoning_tokens": 1 }
        })));
        assert_eq!(mapped["input_tokens"], 4);
        assert_eq!(mapped["output_tokens"], 2);
        assert_eq!(mapped["total_tokens"], 6);
        assert_eq!(mapped["output_tokens_details"]["reasoning_tokens"], 1);
        assert!(mapped.get("prompt_tokens").is_none());

        let zeros = chat_usage_to_responses_usage(None);
        assert_eq!(zeros["input_tokens"], 0);
        assert_eq!(zeros["output_tokens"], 0);
        assert_eq!(zeros["total_tokens"], 0);
        assert_eq!(zeros["output_tokens_details"]["reasoning_tokens"], 0);

        // response.completed must always carry input_tokens for Desktop.
        let completed = json!({
            "type": "response.completed",
            "response": {
                "usage": chat_usage_to_responses_usage(Some(&json!({
                    "prompt_tokens": 10,
                    "completion_tokens": 3
                })))
            }
        });
        let encoded = serde_json::to_string(&completed).unwrap();
        assert!(
            encoded.contains("\"input_tokens\":10"),
            "completed event must expose input_tokens: {encoded}"
        );
    }

    #[test]
    fn stream_chat_request_injects_include_usage() {
        let chat = responses_to_chat_completions(
            &json!({
                "input": "hi",
                "stream": true
            }),
            "deepseek-v4-flash",
        );
        assert_eq!(chat["stream"], true);
        assert_eq!(chat["stream_options"]["include_usage"], true);

        let non_stream = responses_to_chat_completions(
            &json!({ "input": "hi", "stream": false }),
            "deepseek-v4-flash",
        );
        assert!(non_stream.get("stream_options").is_none());
    }

    #[test]
    fn maps_codex_developer_role_to_system_for_deepseek() {
        // Desktop "carefully think" turns inject developer instruction blocks.
        let messages = response_input_to_messages(Some(&json!([
            {
                "type": "message",
                "role": "developer",
                "content": [{"type": "input_text", "text": "Permissions block"}]
            },
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "think carefully"}]
            }
        ])));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Permissions block");
        assert_eq!(messages[1]["role"], "user");
        assert!(
            messages
                .iter()
                .all(|m| m["role"].as_str() != Some("developer")),
            "developer must never reach Chat Completions"
        );

        let chat = responses_to_chat_completions(
            &json!({
                "instructions": "You are Codex.",
                "input": [
                    {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "Collab mode"}]},
                    {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}
                ],
                "stream": false
            }),
            "deepseek-v4-flash",
        );
        let roles: Vec<&str> = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert!(roles.contains(&"system"));
        assert!(roles.contains(&"user"));
        assert!(!roles.contains(&"developer"));
        assert_eq!(
            responses_role_to_chat_role("developer"),
            "system"
        );
        assert_eq!(responses_role_to_chat_role("assistant"), "assistant");
    }

    #[test]
    fn converts_responses_tools_to_chat_function_shape() {
        let tools = responses_tools_to_chat_tools(Some(&json!([
            {
                "type": "function",
                "name": "get_weather",
                "description": "weather",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
            },
            {"type": "local_shell"},
            {
                "type": "function",
                "function": {
                    "name": "already_chat",
                    "parameters": {"type": "object", "properties": {}}
                }
            }
        ])));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["function"]["name"], "get_weather");
        assert_eq!(tools[1]["function"]["name"], "already_chat");
    }

    #[test]
    fn chat_conversion_omits_empty_tools_and_maps_input() {
        let chat = responses_to_chat_completions(
            &json!({
                "model": "ignored",
                "instructions": "You are helpful.",
                "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Hi"}]}],
                "stream": false,
                "tools": [{"type": "local_shell"}],
                "reasoning": {"effort": "high"}
            }),
            "deepseek-v4-flash",
        );
        assert_eq!(chat["model"], "deepseek-v4-flash");
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][1]["content"], "Hi");
        assert!(chat.get("tools").is_none(), "unsupported tools must be dropped");
        assert_eq!(chat["reasoning_effort"], "high");

        let low = responses_to_chat_completions(
            &json!({"input": "hi", "reasoning": {"effort": "none"}}),
            "deepseek-v4-flash",
        );
        assert!(
            low.get("reasoning_effort").is_none(),
            "none/minimal must not emit invalid reasoning_effort"
        );
    }

    #[test]
    fn parse_json_object_body_accepts_double_encoded_objects() {
        let nested = json!({"model": "x", "input": "hi"});
        let encoded = serde_json::to_string(&nested).unwrap();
        let as_string = serde_json::to_vec(&Value::String(encoded)).unwrap();
        let parsed = parse_json_object_body(&Bytes::from(as_string)).expect("decode");
        assert_eq!(parsed["model"], "x");
    }

    #[test]
    fn decode_request_body_unlocks_zstd_json_for_proxy() {
        let nested = json!({"model": "spur-route-test", "input": "hi"});
        let plain = serde_json::to_vec(&nested).unwrap();
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(&plain[..]), 0).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_ENCODING,
            header::HeaderValue::from_static("zstd"),
        );
        let decoded =
            decode_request_body(&mut headers, Bytes::from(compressed)).expect("decompress");
        let parsed = parse_json_object_body(&decoded).expect("json");
        assert_eq!(parsed["model"], "spur-route-test");
        assert!(headers.get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn preserves_v1_base_url() {
        assert_eq!(
            endpoint("https://example.com/v1", "custom", "responses"),
            "https://example.com/v1/responses"
        );
    }

    #[tokio::test]
    async fn stop_waits_for_proxy_task_and_is_idempotent() {
        let stopped = Arc::new(AtomicBool::new(false));
        let task_stopped = Arc::clone(&stopped);
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = receiver.await;
            task_stopped.store(true, Ordering::SeqCst);
        });
        let runtime = ProxyRuntime {
            port: 17_861,
            secret: Arc::new("test".into()),
            shutdown: Mutex::new(Some(shutdown)),
            task: Mutex::new(Some(task)),
        };

        runtime.stop().await;
        runtime.stop().await;
        assert!(stopped.load(Ordering::SeqCst));
    }
}
