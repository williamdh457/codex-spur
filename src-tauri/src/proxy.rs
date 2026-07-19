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
    credentials::SecretMaterial,
    providers,
    storage::{Storage, UsageDelta},
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
}

struct UpstreamAuth {
    credential_id: String,
    token: String,
    account_id: Option<String>,
}

impl ProxyRuntime {
    pub async fn stop(&self) {
        if let Some(sender) = self.shutdown.lock().await.take() {
            let _ = sender.send(());
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

async fn responses(State(state): State<ProxyState>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&headers, &state.secret) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid local proxy token",
        );
    }
    let mut parsed = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(value)) => Value::Object(value),
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be a JSON object",
            )
        }
    };
    let affinity = affinity_key(&headers, &parsed);
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
    map_reasoning(&target, &mut parsed);
    if let Some(object) = parsed.as_object_mut() {
        object.insert("model".into(), Value::String(target.upstream_model.clone()));
    }
    if target.protocol.to_ascii_lowercase().contains("chat") {
        forward_chat_compatible(&state, &target, parsed, affinity.as_deref()).await
    } else {
        forward_responses_compatible(&state, &target, parsed, affinity.as_deref()).await
    }
}

async fn forward_responses_compatible(
    state: &ProxyState,
    target: &RouteTarget,
    request_body: Value,
    affinity_key: Option<&str>,
) -> Response {
    let endpoint = endpoint(&target.base_url, &target.provider_id, "responses");
    for attempt in 0..3 {
        let mut request = state.client.post(&endpoint).json(&request_body);
        let auth = match upstream_auth(state, &target.provider_id, affinity_key).await {
            Ok(Some(auth)) => {
                request = request.bearer_auth(&auth.token);
                if let Some(account_id) = auth.account_id.as_deref() {
                    request = request.header("ChatGPT-Account-Id", account_id);
                }
                Some(auth)
            }
            Ok(None) => None,
            Err(response) => return response,
        };
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                )
            }
        };
        let status = response.status();
        if (status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            && attempt < 2
        {
            if let Some(auth) = auth {
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    let _ = state
                        .storage
                        .mark_credential_health(&auth.credential_id, false, Some("上游认证失败"))
                        .await;
                }
            }
            continue;
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
    affinity_key: Option<&str>,
) -> Response {
    let wants_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let chat_body = json!({
        "model": target.upstream_model,
        "messages": response_input_to_messages(request_body.get("input")),
        "stream": wants_stream,
        "tools": request_body.get("tools").cloned().unwrap_or(Value::Array(Vec::new())),
    });
    let endpoint = endpoint(&target.base_url, &target.provider_id, "chat/completions");
    for attempt in 0..3 {
        let mut request = state.client.post(&endpoint).json(&chat_body);
        let auth = match upstream_auth(state, &target.provider_id, affinity_key).await {
            Ok(Some(auth)) => {
                request = request.bearer_auth(&auth.token);
                if let Some(account_id) = auth.account_id.as_deref() {
                    request = request.header("ChatGPT-Account-Id", account_id);
                }
                Some(auth)
            }
            Ok(None) => None,
            Err(response) => return response,
        };
        let upstream = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                )
            }
        };
        let status = upstream.status();
        if (status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            && attempt < 2
        {
            if let Some(auth) = auth {
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    let _ = state
                        .storage
                        .mark_credential_health(&auth.credential_id, false, Some("上游认证失败"))
                        .await;
                }
            }
            continue;
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
            "usage": payload.get("usage").cloned().unwrap_or(Value::Null)
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
    let body = match upstream.text().await {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_body_error",
                &error.to_string(),
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
    let mut usage = Value::Null;
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
            if let Some(next_usage) = payload.get("usage") {
                usage = next_usage.clone();
            }
        }
    }
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

async fn upstream_auth(
    state: &ProxyState,
    provider_id: &str,
    affinity_key: Option<&str>,
) -> Result<Option<UpstreamAuth>, Response> {
    let pool_id = state
        .storage
        .default_pool_id(provider_id)
        .await
        .map_err(|error| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "account_pool_error",
                &error.to_string(),
            )
        })?;
    let credential = if let Some(pool_id) = pool_id {
        let lease = state
            .storage
            .acquire_lease(&pool_id, affinity_key, 3600)
            .await
            .map_err(|error| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "account_scheduler_error",
                    &error.to_string(),
                )
            })?;
        match lease {
            Some(lease) => state.storage.get_credential(&lease.credential_id).await,
            None => Ok(None),
        }
    } else {
        state.storage.first_healthy_credential(provider_id).await
    }
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
        token,
        account_id: credential.account_id,
    }))
}

fn affinity_key(headers: &HeaderMap, request: &Value) -> Option<String> {
    let raw = headers
        .get("x-codex-session-id")
        .or_else(|| headers.get("x-session-id"))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .or_else(|| {
            request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            request
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })?;
    let mut hasher = Sha256::new();
    hasher.update(b"codex-select-affinity-v1\0");
    hasher.update(raw.as_bytes());
    Some(hex::encode(hasher.finalize()))
}

fn map_reasoning(target: &RouteTarget, request: &mut Value) {
    let Some(codex_effort) = request.pointer("/reasoning/effort").and_then(Value::as_str) else {
        return;
    };
    let profile = providers::reasoning_profile(&target.provider_id, &target.upstream_model);
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

fn response_input_to_messages(input: Option<&Value>) -> Vec<Value> {
    match input {
        Some(Value::String(text)) => vec![json!({"role": "user", "content": text})],
        Some(Value::Array(items)) => {
            let messages = items
                .iter()
                .filter_map(|item| {
                    let role = item.get("role").and_then(Value::as_str)?;
                    let content = item.get("content").map(content_text).unwrap_or_default();
                    Some(json!({"role": role, "content": content}))
                })
                .collect::<Vec<_>>();
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
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn endpoint(base_url: &str, provider_id: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if provider_id == "openai" {
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

pub async fn start(
    catalog: SharedCatalog,
    routes: SharedRoutes,
    storage: Arc<Storage>,
    vault: Arc<SecretVault>,
    preferred_port: u16,
) -> anyhow::Result<ProxyRuntime> {
    let secret = Arc::new(Uuid::new_v4().to_string());
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
    tokio::spawn(async move {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_response_inputs_to_chat_messages() {
        let messages = response_input_to_messages(Some(&json!([
            {"role": "user", "content": [{"type": "input_text", "text": "Hi"}]}
        ])));
        assert_eq!(messages[0]["content"], "Hi");
    }

    #[test]
    fn preserves_v1_base_url() {
        assert_eq!(
            endpoint("https://example.com/v1", "custom", "responses"),
            "https://example.com/v1/responses"
        );
    }
}
