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
    media_sanitizer, providers,
    scheduler::{ScheduleState, SelectionLayer},
    storage::{Lease, Storage, UsageDelta},
    upstream_errors::{
        body_is_usage_or_rate_limit, content_session_seed, is_failover_status, now_unix,
        resolve_rate_limit_cooldown, status_category,
    },
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
        if media_sanitizer::should_strip_images(&target.kind, &target.upstream_model) {
            media_sanitizer::replace_images_with_marker(&mut parsed);
        }
        forward_chat_compatible(&state, &target, parsed, &affinity).await
    } else {
        map_reasoning(&target, &mut parsed);
        // OpenAI kind (官方订阅 / JSON 多账号 / API Key) keeps Codex-native tools.
        // All other kinds (xAI, MiniMax, custom Responses, …) must be ported.
        sanitize_responses_request_for_upstream(&target.kind, &mut parsed);
        if media_sanitizer::should_strip_images(&target.kind, &target.upstream_model) {
            media_sanitizer::replace_images_with_marker(&mut parsed);
        }
        forward_responses_compatible(&state, &target, parsed, &affinity).await
    }
}

async fn forward_responses_compatible(
    state: &ProxyState,
    target: &RouteTarget,
    mut request_body: Value,
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
    let mut media_retry_used = false;
    let started = std::time::Instant::now();
    let mut attempt = 0usize;
    while attempt < max_switches {
        let mut request = state.client.post(&endpoint).json(&request_body);
        let auth = match upstream_auth(state, target, affinity, &exclude).await {
            Ok(Some(auth)) => {
                request = apply_upstream_headers(request, &auth, target);
                Some(auth)
            }
            Ok(None) => {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "no_upstream_credential",
                    "No healthy upstream credential for this route; re-login the account in Codex Spur",
                );
            }
            Err(response) => return response,
        };
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(auth) = auth {
                    record_diag(
                        state,
                        target,
                        &auth,
                        attempt,
                        "transport",
                        false,
                        Some("upstream transport error"),
                        started.elapsed().as_millis() as i64,
                    )
                    .await;
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                    attempt += 1;
                    if attempt < max_switches {
                        exclude.push(auth.credential_id);
                        continue;
                    }
                }
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                );
            }
        };
        let status = response.status();
        let headers = response.headers().clone();
        if is_failover_status(status) {
            let Some(auth) = auth else {
                return passthrough(
                    response,
                    &state.metrics,
                    &state.storage,
                    &target.provider_id,
                    &target.upstream_model,
                )
                .await;
            };
            let body_bytes = response.bytes().await.unwrap_or_default();
            // Text-only gateways often 400 on images; strip once and retry same account.
            if !media_retry_used
                && matches!(status.as_u16(), 400 | 415 | 422)
                && media_sanitizer::is_unsupported_image_error_body(&body_bytes)
                && media_sanitizer::contains_image_blocks(&request_body)
            {
                let stripped = media_sanitizer::replace_images_with_marker(&mut request_body);
                if stripped > 0 {
                    media_retry_used = true;
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                    continue;
                }
            }
            let category = status_category(status);
            let cooldown_applied = handle_upstream_failure(
                state,
                &target.provider_id,
                &auth,
                status,
                &headers,
                Some(body_bytes.as_ref()),
            )
            .await;
            let summary =
                summarize_upstream_error_body(&body_bytes).unwrap_or_else(|| category.to_string());
            record_diag(
                state,
                target,
                &auth,
                attempt,
                category,
                cooldown_applied,
                Some(&summary),
                started.elapsed().as_millis() as i64,
            )
            .await;
            let _ = state.storage.release_lease(&auth.lease_id).await;
            attempt += 1;
            if attempt < max_switches {
                exclude.push(auth.credential_id);
                continue;
            }
            // Last attempt: surface upstream error body.
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY));
            if let Some(ct) = headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
            {
                builder = builder.header(header::CONTENT_TYPE, ct);
            }
            return builder.body(Body::from(body_bytes)).unwrap_or_else(|_| {
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "proxy_response_error",
                    "Failed to build proxy response",
                )
            });
        }
        if let Some(auth) = &auth {
            let category = if status.is_success() {
                "ok"
            } else {
                status_category(status)
            };
            // Non-failover error may still carry usage_limit in body on odd statuses.
            if !status.is_success() {
                let headers = response.headers().clone();
                let body_bytes = response.bytes().await.unwrap_or_default();
                if !media_retry_used
                    && matches!(status.as_u16(), 400 | 415 | 422)
                    && media_sanitizer::is_unsupported_image_error_body(&body_bytes)
                    && media_sanitizer::contains_image_blocks(&request_body)
                {
                    let stripped = media_sanitizer::replace_images_with_marker(&mut request_body);
                    if stripped > 0 {
                        media_retry_used = true;
                        let _ = state.storage.release_lease(&auth.lease_id).await;
                        continue;
                    }
                }
                let cooldown_applied = if body_is_usage_or_rate_limit(&body_bytes) {
                    handle_upstream_failure(
                        state,
                        &target.provider_id,
                        auth,
                        reqwest::StatusCode::TOO_MANY_REQUESTS,
                        &headers,
                        Some(body_bytes.as_ref()),
                    )
                    .await
                } else {
                    false
                };
                let summary = summarize_upstream_error_body(&body_bytes)
                    .unwrap_or_else(|| category.to_string());
                record_diag(
                    state,
                    target,
                    auth,
                    attempt,
                    category,
                    cooldown_applied,
                    Some(&summary),
                    started.elapsed().as_millis() as i64,
                )
                .await;
                let _ = state.storage.release_lease(&auth.lease_id).await;
                let mut builder = Response::builder().status(
                    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                );
                if let Some(ct) = headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                {
                    builder = builder.header(header::CONTENT_TYPE, ct);
                }
                return builder.body(Body::from(body_bytes)).unwrap_or_else(|_| {
                    error_response(
                        StatusCode::BAD_GATEWAY,
                        "proxy_response_error",
                        "Failed to build proxy response",
                    )
                });
            }
            record_diag(
                state,
                target,
                auth,
                attempt,
                category,
                false,
                None,
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
        let auth = match upstream_auth(state, target, affinity, &exclude).await {
            Ok(Some(auth)) => {
                request = apply_upstream_headers(request, &auth, target);
                Some(auth)
            }
            Ok(None) => {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "no_upstream_credential",
                    "No healthy upstream credential for this route; re-login the account in Codex Spur",
                );
            }
            Err(response) => return response,
        };
        let upstream = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(auth) = auth {
                    record_diag(
                        state,
                        target,
                        &auth,
                        attempt,
                        "transport",
                        false,
                        Some("upstream transport error"),
                        started.elapsed().as_millis() as i64,
                    )
                    .await;
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                    if attempt + 1 < max_switches {
                        exclude.push(auth.credential_id);
                        continue;
                    }
                }
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                );
            }
        };
        let status = upstream.status();
        let headers = upstream.headers().clone();
        if is_failover_status(status) {
            let Some(auth) = auth else {
                // No account context — surface upstream response as-is.
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
                let payload = upstream.json::<Value>().await.unwrap_or_else(|_| {
                    json!({"error":{"type":"upstream_error","message":"Upstream request failed"}})
                });
                return (
                    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                    Json(payload),
                )
                    .into_response();
            };
            let body_bytes = upstream.bytes().await.unwrap_or_default();
            let category = status_category(status);
            let cooldown_applied = handle_upstream_failure(
                state,
                &target.provider_id,
                &auth,
                status,
                &headers,
                Some(body_bytes.as_ref()),
            )
            .await;
            record_diag(
                state,
                target,
                &auth,
                attempt,
                category,
                cooldown_applied,
                Some(category),
                started.elapsed().as_millis() as i64,
            )
            .await;
            let _ = state.storage.release_lease(&auth.lease_id).await;
            if attempt + 1 < max_switches {
                exclude.push(auth.credential_id);
                continue;
            }
            return (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(
                    header::CONTENT_TYPE,
                    headers
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("application/json"),
                )],
                body_bytes,
            )
                .into_response();
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
        let reasoning = payload
            .pointer("/choices/0/message/reasoning_content")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .pointer("/choices/0/message/reasoning")
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        let tool_calls = tool_calls_from_chat_message(payload.pointer("/choices/0/message"));
        // Align with CC Switch `response_id_from_chat_id` + type-prefixed item ids.
        let response_id = response_id_from_chat_id(payload.get("id").and_then(Value::as_str));
        let usage = chat_usage_to_responses_usage(payload.get("usage"));
        let output =
            chat_parts_to_responses_output(&response_id, text, reasoning, &tool_calls);
        return Json(json!({
            "id": response_id,
            "object": "response",
            "status": "completed",
            "model": target.upstream_model,
            "output": output,
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

    // Chat Completions SSE → Responses SSE with the full lifecycle Desktop needs.
    // Skipping created/in_progress/item/content_part events makes ChatGPT Desktop
    // drop the turn as "stopped after 0s" even when upstream returned text.
    // Must also surface tool_calls — agent turns are almost always think→tool.
    let parsed = parse_chat_completions_sse(&body);
    let stream = chat_parsed_to_responses_sse(
        &parsed.response_id,
        model_id,
        parsed.created_at,
        &parsed.text,
        &parsed.reasoning,
        &parsed.tool_calls,
        parsed.usage.as_ref(),
    );
    let usage = chat_usage_to_responses_usage(parsed.usage.as_ref());
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
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(stream))
        .unwrap_or_else(|_| {
            error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_response_error",
                "Failed to build streaming response",
            )
        })
}

#[derive(Debug, Default, Clone)]
struct AssembledToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct ParsedChatSse {
    response_id: String,
    created_at: u64,
    text: String,
    reasoning: String,
    tool_calls: Vec<AssembledToolCall>,
    usage: Option<Value>,
}

/// Collect text / reasoning / tool_calls / usage from a buffered Chat Completions SSE body.
fn parse_chat_completions_sse(body: &str) -> ParsedChatSse {
    let mut parsed = ParsedChatSse {
        response_id: format!("resp_{}", Uuid::new_v4()),
        ..ParsedChatSse::default()
    };
    // index → partial tool call (DeepSeek streams name/args across chunks).
    let mut tool_by_index: std::collections::BTreeMap<usize, AssembledToolCall> =
        std::collections::BTreeMap::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(id) = payload.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                // Normalize chatcmpl_* → resp_* so Desktop sees a Responses id
                // (same helper as CC Switch `response_id_from_chat_id`).
                parsed.response_id = response_id_from_chat_id(Some(id));
            }
        }
        if let Some(created) = payload.get("created").and_then(Value::as_u64) {
            parsed.created_at = created;
        }
        if let Some(delta) = payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        {
            if let Some(delta_obj) = delta.get("delta") {
                if let Some(content) = delta_obj.get("content").and_then(Value::as_str) {
                    parsed.text.push_str(content);
                }
                // DeepSeek / reasoners stream thinking into these fields.
                for key in ["reasoning_content", "reasoning", "reasoning_text"] {
                    if let Some(chunk) = delta_obj.get(key).and_then(Value::as_str) {
                        parsed.reasoning.push_str(chunk);
                    }
                }
                merge_chat_tool_call_deltas(
                    &mut tool_by_index,
                    delta_obj.get("tool_calls").and_then(Value::as_array),
                );
            }
            // Some gateways put tool_calls on the final message, not only delta.
            if let Some(message) = delta.get("message") {
                if parsed.text.is_empty() {
                    if let Some(content) = message.get("content").and_then(Value::as_str) {
                        parsed.text.push_str(content);
                    }
                }
                for key in ["reasoning_content", "reasoning", "reasoning_text"] {
                    if parsed.reasoning.is_empty() {
                        if let Some(chunk) = message.get(key).and_then(Value::as_str) {
                            parsed.reasoning.push_str(chunk);
                        }
                    }
                }
                if tool_by_index.is_empty() {
                    for (i, tc) in tool_calls_from_chat_message(Some(message))
                        .into_iter()
                        .enumerate()
                    {
                        tool_by_index.insert(i, tc);
                    }
                }
            }
        }
        if let Some(next_usage) = payload.get("usage").filter(|value| !value.is_null()) {
            parsed.usage = Some(next_usage.clone());
        }
    }
    parsed.tool_calls = tool_by_index
        .into_values()
        .filter(|tc| !tc.name.is_empty())
        .collect();
    if parsed.created_at == 0 {
        parsed.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }
    parsed
}

fn merge_chat_tool_call_deltas(
    tool_by_index: &mut std::collections::BTreeMap<usize, AssembledToolCall>,
    deltas: Option<&Vec<Value>>,
) {
    let Some(deltas) = deltas else {
        return;
    };
    for tc in deltas {
        let index = tc
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let entry = tool_by_index.entry(index).or_default();
        if let Some(id) = tc.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                entry.id = id.to_string();
            }
        }
        let function = tc.get("function").unwrap_or(tc);
        if let Some(name) = function.get("name").and_then(Value::as_str) {
            // First non-empty name wins; later chunks often re-send "".
            if !name.is_empty() && entry.name.is_empty() {
                entry.name = name.to_string();
            }
        }
        if let Some(args) = function.get("arguments").and_then(Value::as_str) {
            entry.arguments.push_str(args);
        }
    }
}

fn tool_calls_from_chat_message(message: Option<&Value>) -> Vec<AssembledToolCall> {
    let Some(message) = message else {
        return Vec::new();
    };
    let Some(items) = message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tc in items {
        let function = tc.get("function").unwrap_or(tc);
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        out.push(AssembledToolCall {
            id: tc
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name,
            arguments: function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string(),
        });
    }
    out
}

fn function_call_item_id(response_id: &str, index: usize) -> String {
    let stem = response_id.strip_prefix("resp_").unwrap_or(response_id);
    format!("fc_{stem}_{index}")
}

fn function_call_call_id(assembled: &AssembledToolCall, response_id: &str, index: usize) -> String {
    if !assembled.id.is_empty() {
        return assembled.id.clone();
    }
    let stem = response_id.strip_prefix("resp_").unwrap_or(response_id);
    format!("call_{stem}_{index}")
}

/// Build Responses `output[]` from Chat Completions parts (non-stream path).
fn chat_parts_to_responses_output(
    response_id: &str,
    text: &str,
    reasoning: &str,
    tool_calls: &[AssembledToolCall],
) -> Vec<Value> {
    let mut output = Vec::new();
    let reasoning = reasoning.trim();
    if !reasoning.is_empty() {
        output.push(json!({
            "id": reasoning_item_id_from_response_id(response_id),
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": reasoning }]
        }));
    }
    for (index, tc) in tool_calls.iter().enumerate() {
        if tc.name.is_empty() {
            continue;
        }
        output.push(json!({
            "id": function_call_item_id(response_id, index),
            "type": "function_call",
            "status": "completed",
            "call_id": function_call_call_id(tc, response_id, index),
            "name": tc.name,
            "arguments": if tc.arguments.is_empty() { "{}" } else { tc.arguments.as_str() }
        }));
    }
    // Always emit a message when there is text, or when there were no tools
    // (empty reply still needs a completed message for Desktop lifecycle).
    if !text.is_empty() || tool_calls.is_empty() {
        output.push(json!({
            "id": message_item_id_from_response_id(response_id),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }]
        }));
    }
    output
}

fn sse_event(event: &str, data: &Value) -> String {
    format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(data).unwrap_or_else(|_| "{}".into())
    )
}

/// Normalize Chat Completions ids into Responses response ids (`resp_…`).
///
/// Behavioral reference: CC Switch `response_id_from_chat_id` (MIT). Independent
/// reimplementation — also strips the common `chatcmpl-` / `chatcmpl_` prefixes.
fn response_id_from_chat_id(id: Option<&str>) -> String {
    let id = id.unwrap_or("codex_select");
    if id.is_empty() {
        return format!("resp_{}", Uuid::new_v4());
    }
    if id.starts_with("resp_") {
        return id.to_string();
    }
    if let Some(rest) = id
        .strip_prefix("chatcmpl-")
        .or_else(|| id.strip_prefix("chatcmpl_"))
    {
        return format!("resp_{rest}");
    }
    format!("resp_{id}")
}

/// Message output-item id. OpenAI requires ids that **begin with `msg`**.
///
/// CC Switch / Nice Switch use type-prefixed item ids for tools (`fc_`, `ctc_`)
/// and reasoning (`rs_`), but their message bridge historically used the
/// suffix form `{response_id}_msg` (e.g. `resp_abc_msg`). Desktop stores that
/// id and later replays it into OpenAI `input[]`, which then 400s:
///
/// ```text
/// Invalid 'input[n].id': 'resp_…_msg'. Expected an ID that begins with 'msg'.
/// ```
///
/// Apply the same type-prefix style to messages: `msg_{stem}`.
fn message_item_id_from_response_id(response_id: &str) -> String {
    if response_id.starts_with("msg") {
        return response_id.to_string();
    }
    let stem = response_id.strip_prefix("resp_").unwrap_or(response_id);
    // Legacy Spur/CC Switch suffix form → canonical prefix form.
    if let Some(stem) = stem.strip_suffix("_msg") {
        return format!("msg_{stem}");
    }
    format!("msg_{stem}")
}

/// Reasoning output-item id (`rs_…`). Matches CC Switch streaming bridge shape.
fn reasoning_item_id_from_response_id(response_id: &str) -> String {
    if response_id.starts_with("rs_") {
        return response_id.to_string();
    }
    let stem = response_id.strip_prefix("resp_").unwrap_or(response_id);
    format!("rs_{stem}")
}

/// Build the Responses SSE lifecycle Desktop expects (CC Switch / Nice Switch shape).
///
/// Sequence for text and/or tool calls (optional reasoning first):
/// `response.created` → `response.in_progress` →
/// [reasoning item lifecycle] →
/// [function_call item lifecycle per tool] →
/// [message item lifecycle when text present, or when no tools] →
/// `response.completed`
///
/// Emitting only `output_text.delta` + `completed` makes ChatGPT Desktop show
/// "stopped after 0s" with no assistant bubble. Emitting reasoning without
/// tools/text used to complete as a silent empty agent turn.
#[cfg(test)]
fn chat_text_to_responses_sse(
    response_id: &str,
    model: &str,
    created_at: u64,
    text: &str,
    reasoning: &str,
    raw_usage: Option<&Value>,
) -> String {
    chat_parsed_to_responses_sse(response_id, model, created_at, text, reasoning, &[], raw_usage)
}

fn chat_parsed_to_responses_sse(
    response_id: &str,
    model: &str,
    created_at: u64,
    text: &str,
    reasoning: &str,
    tool_calls: &[AssembledToolCall],
    raw_usage: Option<&Value>,
) -> String {
    let usage = chat_usage_to_responses_usage(raw_usage);
    let mut out = String::with_capacity(text.len().saturating_mul(2) + 4096);
    let mut output_items: Vec<Value> = Vec::new();
    let mut next_output_index: u32 = 0;

    let base_in_progress = json!({
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "status": "in_progress",
        "model": model,
        "output": [],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
            "output_tokens_details": { "reasoning_tokens": 0 }
        }
    });
    out.push_str(&sse_event(
        "response.created",
        &json!({ "type": "response.created", "response": base_in_progress }),
    ));
    out.push_str(&sse_event(
        "response.in_progress",
        &json!({ "type": "response.in_progress", "response": base_in_progress }),
    ));

    // Optional reasoning item (DeepSeek reasoner / thinking models).
    // Desktop shows summary text here, but there is no OpenAI-signed
    // `encrypted_content`. Replaying this item into OpenAI is stripped by
    // `sanitize_openai_responses_input` (all reasoning dropped on OpenAI path).
    let reasoning = reasoning.trim();
    if !reasoning.is_empty() {
        let item_id = reasoning_item_id_from_response_id(response_id);
        let output_index = next_output_index;
        next_output_index += 1;
        out.push_str(&sse_event(
            "response.output_item.added",
            &json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": item_id,
                    "type": "reasoning",
                    "status": "in_progress",
                    "summary": []
                }
            }),
        ));
        out.push_str(&sse_event(
            "response.reasoning_summary_part.added",
            &json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": item_id,
                "output_index": output_index,
                "summary_index": 0,
                "part": { "type": "summary_text", "text": "" }
            }),
        ));
        out.push_str(&sse_event(
            "response.reasoning_summary_text.delta",
            &json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": item_id,
                "output_index": output_index,
                "summary_index": 0,
                "delta": reasoning
            }),
        ));
        let reasoning_item = json!({
            "id": item_id,
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": reasoning }]
        });
        out.push_str(&sse_event(
            "response.reasoning_summary_text.done",
            &json!({
                "type": "response.reasoning_summary_text.done",
                "item_id": item_id,
                "output_index": output_index,
                "summary_index": 0,
                "text": reasoning
            }),
        ));
        out.push_str(&sse_event(
            "response.reasoning_summary_part.done",
            &json!({
                "type": "response.reasoning_summary_part.done",
                "item_id": item_id,
                "output_index": output_index,
                "summary_index": 0,
                "part": { "type": "summary_text", "text": reasoning }
            }),
        ));
        out.push_str(&sse_event(
            "response.output_item.done",
            &json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": reasoning_item
            }),
        ));
        output_items.push(reasoning_item);
    }

    // Function calls — required for Codex agent turns over Chat Completions.
    for (index, tc) in tool_calls.iter().enumerate() {
        if tc.name.is_empty() {
            continue;
        }
        let item_id = function_call_item_id(response_id, index);
        let call_id = function_call_call_id(tc, response_id, index);
        let arguments = if tc.arguments.is_empty() {
            "{}"
        } else {
            tc.arguments.as_str()
        };
        let output_index = next_output_index;
        next_output_index += 1;
        let item = json!({
            "id": item_id,
            "type": "function_call",
            "status": "in_progress",
            "call_id": call_id,
            "name": tc.name,
            "arguments": ""
        });
        out.push_str(&sse_event(
            "response.output_item.added",
            &json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": item
            }),
        ));
        out.push_str(&sse_event(
            "response.function_call_arguments.delta",
            &json!({
                "type": "response.function_call_arguments.delta",
                "item_id": item_id,
                "output_index": output_index,
                "delta": arguments
            }),
        ));
        out.push_str(&sse_event(
            "response.function_call_arguments.done",
            &json!({
                "type": "response.function_call_arguments.done",
                "item_id": item_id,
                "output_index": output_index,
                "arguments": arguments
            }),
        ));
        let done_item = json!({
            "id": item_id,
            "type": "function_call",
            "status": "completed",
            "call_id": call_id,
            "name": tc.name,
            "arguments": arguments
        });
        out.push_str(&sse_event(
            "response.output_item.done",
            &json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": done_item
            }),
        ));
        output_items.push(done_item);
    }

    // Assistant message — emit when we have text, or when there were no tools
    // (including reasoning-only empty replies so Desktop still gets a message item).
    let has_text = !text.is_empty();
    let has_tools = tool_calls.iter().any(|tc| !tc.name.is_empty());
    if has_text || !has_tools {
        let item_id = message_item_id_from_response_id(response_id);
        let output_index = next_output_index;
        out.push_str(&sse_event(
            "response.output_item.added",
            &json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": item_id,
                    "type": "message",
                    "status": "in_progress",
                    "role": "assistant",
                    "content": []
                }
            }),
        ));
        out.push_str(&sse_event(
            "response.content_part.added",
            &json!({
                "type": "response.content_part.added",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": { "type": "output_text", "text": "", "annotations": [] }
            }),
        ));
        if has_text {
            out.push_str(&sse_event(
                "response.output_text.delta",
                &json!({
                    "type": "response.output_text.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": text
                }),
            ));
        }
        out.push_str(&sse_event(
            "response.output_text.done",
            &json!({
                "type": "response.output_text.done",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": text
            }),
        ));
        out.push_str(&sse_event(
            "response.content_part.done",
            &json!({
                "type": "response.content_part.done",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": { "type": "output_text", "text": text, "annotations": [] }
            }),
        ));
        let message_item = json!({
            "id": item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }]
        });
        out.push_str(&sse_event(
            "response.output_item.done",
            &json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": message_item
            }),
        ));
        output_items.push(message_item);
    }

    let completed_response = json!({
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "status": "completed",
        "model": model,
        "output": output_items,
        "usage": usage
    });
    out.push_str(&sse_event(
        "response.completed",
        &json!({ "type": "response.completed", "response": completed_response }),
    ));
    // Responses API does not use Chat Completions' `data: [DONE]`.
    out
}

struct AffinityInputs {
    previous_response_id: Option<String>,
    session_key: Option<String>,
}

struct UpstreamAuth {
    credential_id: String,
    lease_id: String,
    /// Full `Authorization` header value (`Bearer …` or `AgentAssertion …`).
    authorization: String,
    account_id: Option<String>,
    layer: SelectionLayer,
    sticky_escaped: bool,
}

fn apply_upstream_headers(
    mut request: reqwest::RequestBuilder,
    auth: &UpstreamAuth,
    target: &RouteTarget,
) -> reqwest::RequestBuilder {
    request = request.header(reqwest::header::AUTHORIZATION, &auth.authorization);
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    if target.kind == "openai" && target.base_url.contains("chatgpt.com") {
        request = request
            .header("originator", providers::CODEX_ORIGINATOR)
            .header("version", providers::CODEX_CLIENT_VERSION);
    }
    if target.kind == "kimi" {
        request = request.header(reqwest::header::USER_AGENT, "claude-cli/1.0.0 (Codex Spur)");
    }
    // Grok OAuth subscription CLI proxy rejects otherwise-valid tokens without
    // a supported client identity (observable Grok CLI / Sub2API contract).
    if target.kind == "xai" && providers::xai_base_needs_cli_headers(&target.base_url) {
        request = request
            .header(reqwest::header::USER_AGENT, providers::XAI_CLI_USER_AGENT)
            .header("x-grok-client-version", providers::XAI_CLI_CLIENT_VERSION)
            .header("X-Grok-Client-Version", providers::XAI_CLI_CLIENT_VERSION)
            .header("X-XAI-Token-Auth", "xai-grok-cli");
    }
    request
}

/// Mark account schedule state after an upstream failure.
/// Returns whether a rate-limit cooldown was applied.
async fn handle_upstream_failure(
    state: &ProxyState,
    provider_id: &str,
    auth: &UpstreamAuth,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    body: Option<&[u8]>,
) -> bool {
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
        return false;
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
        return false;
    }
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        let _ = state
            .storage
            .mark_schedule_state(
                &auth.credential_id,
                ScheduleState::Entitlement,
                false,
                Some("上游支付/余额失败 (402)"),
                None,
            )
            .await;
        return false;
    }
    let is_rate = status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || body.is_some_and(body_is_usage_or_rate_limit);
    if is_rate {
        let default_secs = state
            .storage
            .default_429_cooldown_secs(provider_id)
            .await
            .unwrap_or(30);
        let decision = resolve_rate_limit_cooldown(headers, body, default_secs, now_unix());
        let reason = format!("上游限流 ({})", decision.reason);
        let _ = state
            .storage
            .apply_rate_limit_until(&auth.credential_id, decision.cooldown_until, &reason)
            .await;
        return true;
    }
    false
}

async fn upstream_auth(
    state: &ProxyState,
    target: &RouteTarget,
    affinity: &AffinityInputs,
    exclude: &[String],
) -> Result<Option<UpstreamAuth>, Response> {
    let provider_id = target.provider_id.as_str();
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
        // No eligible pool member: try healthy, then any refreshable OAuth (recovery).
        let credential = state
            .storage
            .first_refreshable_oauth_credential(provider_id)
            .await
            .map_err(|error| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential_store_error",
                    &error.to_string(),
                )
            })?;
        let Some(credential) = credential else {
            // API-key providers without oauth: fall back to first healthy.
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
                target,
                &Lease {
                    id: format!("ephemeral-{}", Uuid::new_v4()),
                    credential_id: credential.id.clone(),
                    layer: crate::scheduler::SelectionLayer::LoadBalance,
                    sticky_escaped: false,
                },
                credential,
            )
            .await;
        };
        return decrypt_auth(
            state,
            target,
            &Lease {
                id: format!("ephemeral-{}", Uuid::new_v4()),
                credential_id: credential.id.clone(),
                layer: crate::scheduler::SelectionLayer::LoadBalance,
                sticky_escaped: false,
            },
            credential,
        )
        .await;
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
    decrypt_auth(state, target, &lease, credential).await
}

#[allow(clippy::result_large_err)]
async fn decrypt_auth(
    state: &ProxyState,
    target: &RouteTarget,
    lease: &Lease,
    mut credential: crate::storage::StoredCredential,
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
    let mut secret = SecretMaterial::from_json_bytes(plaintext.as_slice()).map_err(|error| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential_decode_error",
            &error.to_string(),
        )
    })?;
    let mut expires_at_for_store: Option<i64> = None;
    let mut secret_dirty = false;
    let mut refreshed_ok = false;
    // Prefer explicit route kind; fall back to known xAI hosts only (never treat empty base as xAI).
    let base_lower = target.base_url.to_ascii_lowercase();
    let is_xai = target.kind == "xai"
        || base_lower.contains("cli-chat-proxy.grok.com")
        || base_lower.contains("api.x.ai");

    // Refresh OAuth access tokens before they expire. Branch by upstream kind:
    // xAI/Grok uses auth.x.ai; ChatGPT uses the OpenAI OAuth token endpoint.
    if secret.api_key.is_none() {
        if let Some(refresh) = secret.refresh_token.clone() {
            let needs = match secret.access_token.as_deref() {
                Some(access) => crate::openai_oauth::access_token_needs_refresh(
                    access,
                    credential.expires_at,
                ),
                // Missing access token — must refresh when we still have refresh_token.
                None => true,
            };
            if needs {
                let refresh_result = if is_xai {
                    crate::xai_oauth::refresh_xai_tokens(&refresh)
                        .await
                        .map(|t| {
                            (
                                t.access_token,
                                t.refresh_token,
                                t.id_token,
                                t.account_id,
                                t.expires_at,
                            )
                        })
                } else if target.kind == "openai"
                    || target.base_url.contains("chatgpt.com")
                    || target.base_url.contains("openai.com")
                {
                    crate::openai_oauth::refresh_chatgpt_tokens(&refresh, secret.id_token.as_deref())
                        .await
                        .map(|t| {
                            (
                                t.access_token,
                                t.refresh_token,
                                t.id_token,
                                t.account_id,
                                t.expires_at,
                            )
                        })
                } else {
                    // Unknown oauth upstream — do not call ChatGPT refresh by default.
                    Err("unsupported oauth refresh for this provider kind".into())
                };

                match refresh_result {
                    Ok((access_token, new_refresh, id_token, account_id, expires_at)) => {
                        secret.access_token = Some(access_token);
                        if let Some(id_token) = id_token {
                            secret.id_token = Some(id_token);
                        }
                        if let Some(new_refresh) = new_refresh {
                            secret.refresh_token = Some(new_refresh);
                        }
                        if !account_id.trim().is_empty() {
                            credential.account_id = Some(account_id);
                        }
                        expires_at_for_store = expires_at;
                        secret_dirty = true;
                        refreshed_ok = true;
                    }
                    Err(error) => {
                        let lower = error.to_lowercase();
                        let hard_fail = lower.contains("invalid_grant")
                            || lower.contains("invalid_token")
                            || lower.contains("expired")
                            || lower.contains("revoked");
                        if hard_fail {
                            let msg = if is_xai {
                                "Grok OAuth refresh 失败，请在 Spur 中重新登录该 Grok 账号"
                            } else {
                                "OAuth refresh 失败，请重新登录官方订阅"
                            };
                            let _ = state
                                .storage
                                .mark_schedule_state(
                                    &credential.id,
                                    ScheduleState::AuthInvalid,
                                    false,
                                    Some(msg),
                                    None,
                                )
                                .await;
                        }
                        // Fall through with existing access token; upstream may still 401.
                        let _ = error;
                    }
                }
            }
        }
    }

    // Recover missing ChatGPT account_id (partial JSON imports) for upstream headers.
    if !is_xai
        && credential
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        if let Some(access) = secret.access_token.as_deref() {
            if let Ok(id) = crate::openai_oauth::ensure_chatgpt_account_id(
                access,
                secret.id_token.as_deref(),
                None,
            )
            .await
            {
                credential.account_id = Some(id);
                secret_dirty = true;
            }
        }
    }

    // Agent Identity: register/sign a task and use AgentAssertion (no OAuth bearer).
    if let Some(mut agent_key) = crate::openai_agent_identity::agent_key_from_secret(&secret) {
        if agent_key.task_id.as_deref().map(str::trim).unwrap_or("").is_empty() {
            match crate::openai_agent_identity::register_agent_task(&agent_key).await {
                Ok(task_id) => {
                    agent_key.task_id = Some(task_id.clone());
                    secret.task_id = Some(task_id);
                    secret_dirty = true;
                }
                Err(error) => {
                    return Err(error_response(
                        StatusCode::UNAUTHORIZED,
                        "agent_task_register_failed",
                        &error.to_string(),
                    ));
                }
            }
        }
        let task_id = agent_key.task_id.clone().unwrap_or_default();
        let authorization =
            match crate::openai_agent_identity::authorization_header_for_agent_task(
                &agent_key, &task_id,
            ) {
                Ok(value) => value,
                Err(error) => {
                    return Err(error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "agent_assertion_failed",
                        &error.to_string(),
                    ));
                }
            };
        if secret_dirty {
            if let Ok(json) = serde_json::to_vec(&serde_json::json!({
                "access_token": secret.access_token,
                "refresh_token": secret.refresh_token,
                "id_token": secret.id_token,
                "session_token": secret.session_token,
                "api_key": secret.api_key,
                "agent_runtime_id": secret.agent_runtime_id,
                "agent_private_key": secret.agent_private_key,
                "task_id": secret.task_id,
            })) {
                if let Ok(envelope) = state.vault.encrypt(&credential.id, 1, json.as_slice()) {
                    if let Ok(envelope_json) = serde_json::to_string(&envelope) {
                        let account_id_for_store = credential
                            .account_id
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let _ = state
                            .storage
                            .update_credential_secret(
                                &credential.id,
                                &envelope_json,
                                None,
                                account_id_for_store,
                            )
                            .await;
                    }
                }
            }
        }
        return Ok(Some(UpstreamAuth {
            credential_id: credential.id,
            lease_id: lease.id.clone(),
            authorization,
            account_id: credential.account_id,
            layer: lease.layer,
            sticky_escaped: lease.sticky_escaped,
        }));
    }

    if secret_dirty {
        if let Ok(json) = serde_json::to_vec(&serde_json::json!({
            "access_token": secret.access_token,
            "refresh_token": secret.refresh_token,
            "id_token": secret.id_token,
            "session_token": secret.session_token,
            "api_key": secret.api_key,
            "agent_runtime_id": secret.agent_runtime_id,
            "agent_private_key": secret.agent_private_key,
            "task_id": secret.task_id,
        })) {
            if let Ok(envelope) = state.vault.encrypt(&credential.id, 1, json.as_slice()) {
                if let Ok(envelope_json) = serde_json::to_string(&envelope) {
                    let account_id_for_store = credential
                        .account_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let _ = state
                        .storage
                        .update_credential_secret(
                            &credential.id,
                            &envelope_json,
                            expires_at_for_store,
                            account_id_for_store,
                        )
                        .await;
                }
            }
        }
    }

    // Successful refresh heals auth_invalid / unhealthy so the scheduler can
    // select this account again on subsequent turns.
    if refreshed_ok {
        let _ = state
            .storage
            .mark_schedule_state(
                &credential.id,
                ScheduleState::Ready,
                true,
                None,
                None,
            )
            .await;
    }

    let token = secret
        .api_key
        .or(secret.access_token)
        .or(secret.session_token);
    Ok(token.map(|token| UpstreamAuth {
        credential_id: credential.id,
        lease_id: lease.id.clone(),
        authorization: format!("Bearer {token}"),
        account_id: credential.account_id,
        layer: lease.layer,
        sticky_escaped: lease.sticky_escaped,
    }))
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
    let max_events = state.storage.diagnostics_max_events().await.unwrap_or(200);
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

    // Explicit session signals (Sub2API-like order).
    let session_raw = headers
        .get("x-codex-session-id")
        .or_else(|| headers.get("x-session-id"))
        .or_else(|| headers.get("session_id"))
        .or_else(|| headers.get("conversation_id"))
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

    let session_key = if let Some(raw) = session_raw {
        let mut hasher = Sha256::new();
        hasher.update(b"codex-select-session-v1\0");
        hasher.update(raw.as_bytes());
        Some(hex::encode(hasher.finalize()))
    } else if let Some(seed) = content_session_seed(request) {
        // Content-derived fallback keeps multi-turn sticky when clients omit session headers.
        let mut hasher = Sha256::new();
        hasher.update(b"codex-select-content-seed-v1\0");
        hasher.update(seed.as_bytes());
        Some(hex::encode(hasher.finalize()))
    } else {
        None
    };

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

/// Tool `type` values accepted by xAI's Responses API (from upstream 422 enum).
const XAI_RESPONSES_TOOL_TYPES: &[&str] = &[
    "function",
    "web_search",
    "x_search",
    "image_generation",
    "collections_search",
    "file_search",
    "code_execution",
    "code_interpreter",
    "mcp",
    "shell",
];

/// Conservative portable set for non-OpenAI Responses hosts (MiniMax, custom, …).
const GENERIC_RESPONSES_TOOL_TYPES: &[&str] = &[
    "function",
    "web_search",
    "file_search",
    "code_interpreter",
    "code_execution",
    "mcp",
    "shell",
];

/// Chat Completions only understands function tools (DeepSeek / Kimi / most gateways).
const CHAT_COMPLETIONS_TOOL_TYPES: &[&str] = &["function"];

/// OpenAI kind covers 官方订阅 / JSON 多账号导入 / API Key — keep Codex-native shapes.
fn keeps_codex_native_tools(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("openai")
}

/// Allowed Responses tool types for a non-OpenAI kind.
fn responses_tool_types_for_kind(kind: &str) -> &'static [&'static str] {
    match kind.to_ascii_lowercase().as_str() {
        "xai" => XAI_RESPONSES_TOOL_TYPES,
        // kimi / deepseek use Chat Completions (handled elsewhere); if a route
        // is mis-stamped as Responses, still use the conservative set.
        _ => GENERIC_RESPONSES_TOOL_TYPES,
    }
}

/// Port Codex Desktop `tools[]` rows into an allow-list for a third-party host.
///
/// - keeps rows whose `type` is in `allowed`
/// - remaps `local_shell` → `shell` when `shell` is allowed
/// - flattens nested tools under Codex `namespace` groups
/// - drops `custom` / empty namespaces / other Codex-only kinds
fn port_codex_tools(tools: &[Value], allowed: &[&str]) -> Vec<Value> {
    let mut kept = Vec::with_capacity(tools.len());
    for tool in tools {
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");

        if tool_type == "local_shell" && allowed.contains(&"shell") {
            let mut remapped = tool.clone();
            if let Some(object) = remapped.as_object_mut() {
                object.insert("type".into(), Value::String("shell".into()));
            }
            kept.push(remapped);
            continue;
        }

        if !tool_type.is_empty() && allowed.contains(&tool_type) {
            kept.push(tool.clone());
            continue;
        }

        // Already Chat Completions shaped `{type:function, function:{name,…}}`.
        if tool.get("function").is_some() && allowed.contains(&"function") {
            let mut row = tool.clone();
            if tool_type.is_empty() {
                if let Some(object) = row.as_object_mut() {
                    object.insert("type".into(), Value::String("function".into()));
                }
            }
            kept.push(row);
            continue;
        }

        // Flatten Codex namespace groups: keep portable nested tools only.
        if tool_type == "namespace" {
            if let Some(nested) = tool.get("tools").and_then(Value::as_array) {
                kept.extend(port_codex_tools(nested, allowed));
            }
            continue;
        }

        // Freeform Responses function without explicit type, but never treat
        // named non-function kinds (namespace/custom/…) as functions.
        if tool_type.is_empty()
            && tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| !n.is_empty())
            && allowed.contains(&"function")
        {
            let mut row = tool.clone();
            if let Some(object) = row.as_object_mut() {
                object.insert("type".into(), Value::String("function".into()));
            }
            kept.push(row);
        }
        // Drop: custom, apply_patch, empty namespace, unknown kinds.
    }
    kept
}

/// Responses-path port for every non-OpenAI kind (xAI / MiniMax / custom / …).
///
/// OpenAI kind (官方订阅 / JSON 多账号 / API Key) keeps Codex-native shapes,
/// but still sanitizes Desktop history that was poisoned by Chat-bridge turns
/// (bad message ids, foreign reasoning/`encrypted_content`, dead item refs).
/// Non-OpenAI kinds share this pipeline: strip Codex-only tools/tool_choice,
/// drop **all** reasoning (OpenAI ciphertext is not xAI-decryptable), and
/// clamp fields so future subscriptions do not re-introduce 422s.
fn sanitize_responses_request_for_upstream(kind: &str, request: &mut Value) {
    if keeps_codex_native_tools(kind) {
        sanitize_openai_responses_input(request);
        return;
    }
    sanitize_responses_tools_for_upstream(kind, request);
    sanitize_responses_tool_choice_for_upstream(kind, request);
    sanitize_responses_input_for_upstream(request);
    strip_unsupported_responses_fields(request);
    clamp_responses_fields_for_kind(kind, request);
}

/// Rewrite one message-like item's id to OpenAI's `msg…` prefix, if needed.
fn rewrite_openai_message_item_id(item: &mut Value) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let has_role = item.get("role").and_then(Value::as_str).is_some();
    let is_message = item_type == "message" || (item_type.is_empty() && has_role);
    if !is_message {
        return;
    }
    let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    // OpenAI wording: "begins with 'msg'".
    if id.starts_with("msg") {
        return;
    }
    let rewritten = message_item_id_from_response_id(&id);
    if rewritten == id {
        return;
    }
    if let Some(object) = item.as_object_mut() {
        object.insert("id".into(), Value::String(rewritten));
    }
}

/// Sanitize Desktop-replayed `input` for OpenAI Responses (mixed-model threads).
///
/// Layers:
/// 1. Message ids must begin with `msg` (legacy Spur/CC Switch used `resp_…_msg`).
/// 2. Drop **all** reasoning items (foreign encrypted_content decrypts fail;
///    bridge-only reasoning 404s under `store=false`).
/// 3. When `store` is not `true`, drop `item_reference` (unresolvable offline).
/// 4. If any input item was dropped, strip `previous_response_id` so OpenAI does
///    not chase a Spur-synthetic or already-invalid server id.
fn sanitize_openai_responses_input(request: &mut Value) {
    let store_is_true = request.get("store").and_then(Value::as_bool) == Some(true);
    let drop_item_references = !store_is_true;

    let mut dropped_any = false;
    if let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) {
        let mut next = Vec::with_capacity(items.len());
        for mut item in items.drain(..) {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "reasoning" => {
                    // Never portable across Grok/DeepSeek/Kimi ↔ OpenAI.
                    dropped_any = true;
                }
                "item_reference" if drop_item_references => {
                    dropped_any = true;
                }
                _ => {
                    rewrite_openai_message_item_id(&mut item);
                    next.push(item);
                }
            }
        }
        if let Some(object) = request.as_object_mut() {
            object.insert("input".into(), Value::Array(next));
        }
    }

    if dropped_any {
        if let Some(object) = request.as_object_mut() {
            object.remove("previous_response_id");
        }
    }
}

/// Back-compat name used by older call sites / mental model: message-id rewrite
/// only. Prefer [`sanitize_openai_responses_input`] for the full OpenAI path.
#[cfg(test)]
fn sanitize_openai_input_message_ids(request: &mut Value) {
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items.iter_mut() {
        rewrite_openai_message_item_id(item);
    }
}

/// Drop or remap Codex-only tool rows so third-party Responses APIs do not 422.
///
/// Observed failure (xAI): `unknown variant namespace, expected one of function,
/// web_search, x_search, image_generation, collections_search, file_search,
/// code_execution, code_interpreter, mcp, shell`.
fn sanitize_responses_tools_for_upstream(kind: &str, request: &mut Value) {
    if keeps_codex_native_tools(kind) {
        return;
    }
    let allowed = responses_tool_types_for_kind(kind);
    let Some(items) = request
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
    else {
        return;
    };
    let kept = port_codex_tools(&items, allowed);
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if kept.is_empty() {
        object.remove("tools");
    } else {
        object.insert("tools".into(), Value::Array(kept));
    }
}

/// Align or drop `tool_choice` after tools were filtered.
///
/// Codex may send `{"type":"namespace",…}` or a function name that no longer
/// exists after namespace/custom rows were dropped — upstream then 422s.
fn sanitize_responses_tool_choice_for_upstream(kind: &str, request: &mut Value) {
    if keeps_codex_native_tools(kind) {
        return;
    }
    let Some(choice) = request.get("tool_choice").cloned() else {
        return;
    };
    let tools = request
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if should_drop_tool_choice(&choice, &tools, responses_tool_types_for_kind(kind)) {
        if let Some(object) = request.as_object_mut() {
            object.remove("tool_choice");
        }
    }
}

fn should_drop_tool_choice(choice: &Value, tools: &[Value], allowed: &[&str]) -> bool {
    if tools.is_empty() {
        return true;
    }
    match choice {
        Value::String(s) => {
            // auto/none/required are fine when tools remain.
            !matches!(s.as_str(), "auto" | "none" | "required")
                && !tools.iter().any(|t| {
                    t.get("name").and_then(Value::as_str) == Some(s.as_str())
                        || t.pointer("/function/name").and_then(Value::as_str) == Some(s.as_str())
                })
        }
        Value::Object(map) => {
            let choice_type = map.get("type").and_then(Value::as_str).unwrap_or("");
            if choice_type.is_empty() {
                return false;
            }
            if !allowed.contains(&choice_type) {
                return true;
            }
            if choice_type == "function" {
                let name = map
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        map.get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or("");
                if name.is_empty() {
                    return false;
                }
                return !tools.iter().any(|tool| {
                    tool.get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("function")
                        == "function"
                        && (tool.get("name").and_then(Value::as_str) == Some(name)
                            || tool.pointer("/function/name").and_then(Value::as_str) == Some(name))
                });
            }
            false
        }
        _ => false,
    }
}

/// Drop Codex-only / non-portable input carriers that third-party Responses hosts reject.
///
/// - `additional_tools`: Responses Lite private carrier (xAI ModelInput fails)
/// - **all** `reasoning` items: OpenAI/xAI `encrypted_content` is not portable across
///   providers (official GPT → Grok fails with "Could not decrypt encrypted_content");
///   summary-only bridge reasoning is also dropped for symmetry with the OpenAI path
/// - `item_reference` when `store` is not true (unresolvable on foreign hosts)
/// - If anything was dropped, strip `previous_response_id` so affinity does not chase
///   an OpenAI (or other foreign) response id on xAI/MiniMax/etc.
fn sanitize_responses_input_for_upstream(request: &mut Value) {
    let store_is_true = request.get("store").and_then(Value::as_bool) == Some(true);
    let drop_item_references = !store_is_true;

    let mut dropped_any = false;
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let mut next = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "additional_tools" => {
                dropped_any = true;
            }
            // Symmetric with sanitize_openai_responses_input: foreign encrypted
            // reasoning cannot be decrypted by the current upstream (e.g. OpenAI
            // gAAAAA… blobs on xAI → invalid-argument decrypt error).
            "reasoning" => {
                dropped_any = true;
            }
            "item_reference" if drop_item_references => {
                dropped_any = true;
            }
            _ => {
                next.push(item);
            }
        }
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("input".into(), Value::Array(next));
        if dropped_any {
            object.remove("previous_response_id");
        }
    }
}

/// Fields known to 422 on non-OpenAI Responses hosts (xAI and peers).
const UNSUPPORTED_RESPONSES_TOP_LEVEL: &[&str] = &["prompt_cache_retention", "safety_identifier"];

fn strip_unsupported_responses_fields(request: &mut Value) {
    if let Some(object) = request.as_object_mut() {
        for key in UNSUPPORTED_RESPONSES_TOP_LEVEL {
            object.remove(*key);
        }
    }
    strip_json_key_recursive(request, "external_web_access");
}

fn strip_json_key_recursive(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.remove(key);
            for child in map.values_mut() {
                strip_json_key_recursive(child, key);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_json_key_recursive(item, key);
            }
        }
        _ => {}
    }
}

/// Kind/model-specific clamps. Defaults are conservative; table is easy to extend.
fn clamp_responses_fields_for_kind(kind: &str, request: &mut Value) {
    if !kind.eq_ignore_ascii_case("xai") {
        return;
    }
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let Some(object) = request.as_object_mut() else {
        return;
    };
    // Composer models reject reasoning effort knobs.
    if model.contains("composer") {
        object.remove("reasoning");
        object.remove("reasoning_effort");
        object.remove("reasoningEffort");
    }
    // grok-4.5 has been observed to reject classic chat penalty/stop fields.
    if model.contains("grok-4.5") || model == "grok-4.5" {
        for key in [
            "presence_penalty",
            "presencePenalty",
            "frequency_penalty",
            "frequencyPenalty",
            "stop",
        ] {
            object.remove(key);
        }
    }
}

/// Extract a short, secret-free upstream error message for diagnostics.
fn summarize_upstream_error_body(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_slice(body).ok()?;
    let message = parsed
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| parsed.get("error").and_then(Value::as_str))
        .or_else(|| parsed.get("message").and_then(Value::as_str))
        .unwrap_or("");
    let trimmed = message.trim();
    if trimmed.is_empty() {
        // Fall back to a tiny raw snippet (no headers/tokens in JSON error bodies).
        let raw = String::from_utf8_lossy(body);
        let snippet: String = raw.chars().take(200).collect();
        if snippet.trim().is_empty() {
            return None;
        }
        return Some(snippet);
    }
    Some(trimmed.chars().take(200).collect())
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
    let is_stream = chat.get("stream").and_then(Value::as_bool).unwrap_or(false);
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
            .insert("tools".into(), Value::Array(tools.clone()));
        if let Some(tool_choice) = request_body.get("tool_choice") {
            // Drop choice that points at namespace / removed tools (same rules as Responses port).
            if !should_drop_tool_choice(tool_choice, &tools, CHAT_COMPLETIONS_TOOL_TYPES) {
                chat.as_object_mut()
                    .expect("chat object")
                    .insert("tool_choice".into(), tool_choice.clone());
            }
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
            .filter_map(|part| {
                part.as_str()
                    .map(str::to_string)
                    .or_else(|| part.get("text").and_then(Value::as_str).map(str::to_string))
            })
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
            // Batch consecutive function_call items, and hold assistant text so we can
            // emit a single Chat Completions assistant turn:
            //   { role: assistant, content, tool_calls? }
            //
            // Desktop / chat-bridge history may order items either as
            //   message → function_call → function_call_output  (Grok-native)
            // or
            //   function_call → message → function_call_output  (chat SSE: tools then text)
            // Splitting those into two assistant messages breaks Kimi/DeepSeek:
            //   "tool_calls must be followed by tool messages" (e.g. exec_command:N).
            let mut pending_tool_calls: Vec<Value> = Vec::new();
            let mut pending_assistant_text: Option<String> = None;

            let flush_assistant_turn =
                |messages: &mut Vec<Value>,
                 pending_tools: &mut Vec<Value>,
                 pending_text: &mut Option<String>| {
                    let has_tools = !pending_tools.is_empty();
                    let text = pending_text.take().unwrap_or_default();
                    if !has_tools && text.is_empty() {
                        return;
                    }
                    let content = if text.is_empty() {
                        // OpenAI-compatible tool turns use null content when only tools.
                        Value::Null
                    } else {
                        Value::String(text)
                    };
                    let mut msg = json!({
                        "role": "assistant",
                        "content": content,
                    });
                    if has_tools {
                        msg.as_object_mut()
                            .expect("assistant object")
                            .insert("tool_calls".into(), Value::Array(std::mem::take(pending_tools)));
                    }
                    messages.push(msg);
                };

            let append_pending_text = |pending_text: &mut Option<String>, text: String| {
                if text.is_empty() {
                    if pending_text.is_none() {
                        *pending_text = Some(String::new());
                    }
                    return;
                }
                match pending_text {
                    Some(existing) => {
                        if !existing.is_empty() {
                            existing.push('\n');
                        }
                        existing.push_str(&text);
                    }
                    None => *pending_text = Some(text),
                }
            };

            for item in items {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                match item_type {
                    // Encrypted / foreign reasoning is not meaningful on Chat Completions.
                    "reasoning" | "web_search_call" | "item_reference" => continue,
                    "custom_tool_call" | "custom_tool_call_output" => {
                        // Not mapped in v1; drop rather than poison history.
                        continue;
                    }
                    "function_call" => {
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if call_id.is_empty() || name.is_empty() {
                            continue;
                        }
                        let arguments = match item.get("arguments") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                            None => "{}".into(),
                        };
                        pending_tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": arguments
                            }
                        }));
                    }
                    "function_call_output" => {
                        // Close the assistant turn (text + tools) before tool results.
                        flush_assistant_turn(
                            &mut messages,
                            &mut pending_tool_calls,
                            &mut pending_assistant_text,
                        );
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if call_id.is_empty() {
                            continue;
                        }
                        let output = match item.get("output") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                            None => String::new(),
                        };
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": output
                        }));
                    }
                    _ => {
                        let raw_role =
                            item.get("role").and_then(Value::as_str).unwrap_or("user");
                        let role = responses_role_to_chat_role(raw_role);
                        let text = if let Some(content) = item.get("content") {
                            content_text(content)
                        } else if let Some(text) = item.get("text").and_then(Value::as_str) {
                            text.to_string()
                        } else {
                            String::new()
                        };

                        if role == "assistant" {
                            // Merge into the open tool turn when tools are already pending
                            // (function_call → message → function_call_output).
                            if !pending_tool_calls.is_empty() {
                                append_pending_text(&mut pending_assistant_text, text);
                                continue;
                            }
                            // Hold text so a following function_call can share one assistant
                            // message (message → function_call → function_call_output).
                            // A second assistant text without tools flushes the previous one.
                            if pending_assistant_text.is_some() {
                                flush_assistant_turn(
                                    &mut messages,
                                    &mut pending_tool_calls,
                                    &mut pending_assistant_text,
                                );
                            }
                            if !text.is_empty() || item.get("content").is_some() {
                                // Preserve empty assistant placeholders when content key existed.
                                append_pending_text(&mut pending_assistant_text, text);
                            }
                            continue;
                        }

                        // user / system / tool-as-message: close any open assistant turn first.
                        flush_assistant_turn(
                            &mut messages,
                            &mut pending_tool_calls,
                            &mut pending_assistant_text,
                        );
                        if !text.is_empty() || role == "assistant" {
                            messages.push(json!({"role": role, "content": text}));
                        }
                    }
                }
            }
            flush_assistant_turn(
                &mut messages,
                &mut pending_tool_calls,
                &mut pending_assistant_text,
            );
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
///
/// Used for Kimi / DeepSeek / other Chat Completions routes. Applies the same
/// Codex porting rules as non-OpenAI Responses (flatten `namespace`, drop
/// `local_shell` / `custom` / …) then rewrites freeform function rows into the
/// nested `function` object Chat Completions expects.
fn responses_tools_to_chat_tools(tools: Option<&Value>) -> Vec<Value> {
    let Some(Value::Array(items)) = tools else {
        return Vec::new();
    };
    // Chat Completions only accepts function tools; shell/web_search are not portable.
    let portable = port_codex_tools(items, CHAT_COMPLETIONS_TOOL_TYPES);
    let mut out = Vec::with_capacity(portable.len());
    for tool in portable {
        // Already Chat Completions shaped.
        if tool.get("function").is_some() {
            let mut row = tool;
            if row
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
            {
                if let Some(object) = row.as_object_mut() {
                    object.insert("type".into(), Value::String("function".into()));
                }
            }
            out.push(row);
            continue;
        }
        // Responses freeform function: {type:function, name, description, parameters}
        let Some(name) = tool.get("name").and_then(Value::as_str).map(str::to_string) else {
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
    let is_sse = content_type
        .as_deref()
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"));

    // True streaming for Responses SSE: forward chunks as they arrive so Desktop
    // sees first token promptly (avoid buffering the whole upstream stream).
    if is_sse && status.is_success() {
        // Usage is approximate for live streams (full JSON parse only on buffered paths).
        metrics.output_tokens.fetch_add(1, Ordering::Relaxed);
        let _ = storage
            .record_usage(
                provider_id,
                model_id,
                &UsageDelta {
                    request_count: 0,
                    input_tokens: 0,
                    output_tokens: 1,
                    cache_observations: 0,
                    cache_hits: 0,
                    failed_requests: 0,
                },
            )
            .await;
        let stream = response.bytes_stream();
        let mapped = futures_util::StreamExt::map(stream, |chunk| {
            chunk.map_err(|error| std::io::Error::other(error.to_string()))
        });
        let mut builder = Response::builder().status(status);
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        builder = builder.header(header::CACHE_CONTROL, "no-cache");
        return builder.body(Body::from_stream(mapped)).unwrap_or_else(|_| {
            error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_response_error",
                "Failed to build streaming proxy response",
            )
        });
    }

    match response.bytes().await {
        Ok(bytes) => {
            // Non-SSE success that is still a JSON error envelope (some gateways).
            if status.is_success() {
                if let Ok(body) = serde_json::from_slice::<Value>(&bytes) {
                    if body.get("error").is_some() && body.get("output").is_none() {
                        let message = body
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .or_else(|| body.get("error").and_then(Value::as_str))
                            .unwrap_or("Upstream returned an error object");
                        return error_response(
                            StatusCode::BAD_GATEWAY,
                            "upstream_error_envelope",
                            message,
                        );
                    }
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
                                failed_requests: 0,
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
                                failed_requests: 0,
                            },
                        )
                        .await;
                }
            } else if let Ok(body) = serde_json::from_slice::<Value>(&bytes) {
                metrics.record_response(&body);
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


/// Kimi Desktop agent-gw compatible entry (experimental).
/// Maps model aliases (`spur-…`) → Spur route slugs via alias map, then reuses
/// the Chat Completions upstream path. Binds only on the existing localhost proxy.
async fn kimi_coding_chat_completions(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    kimi_coding_chat_completions_inner(state, headers, body).await
}

async fn kimi_coding_chat_completions_v1(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Some clients call /coding/v1/v1/chat/completions when base already ends with /v1.
    kimi_coding_chat_completions_inner(state, headers, body).await
}

async fn kimi_coding_chat_completions_inner(
    state: ProxyState,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &state.secret) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid local proxy token for Kimi-compat gateway",
        );
    }
    let mut parsed = match serde_json::from_slice::<Value>(&body) {
        Ok(v) => v,
        Err(err) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Invalid chat body: {err}"),
            );
        }
    };
    let model = parsed
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if model.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing_model", "model is required");
    }
    let route_slug = resolve_kimi_model_to_route(&model);
    let target = state.routes.read().await.get(&route_slug).cloned();
    let Some(target) = target else {
        return error_response(
            StatusCode::NOT_FOUND,
            "unknown_route",
            &format!(
                "No Spur route for Kimi model `{model}` (mapped `{route_slug}`). Publish routes in Codex Spur first."
            ),
        );
    };
    if let Some(object) = parsed.as_object_mut() {
        object.insert("model".into(), Value::String(target.upstream_model.clone()));
    }
    let affinity = affinity_inputs(&headers, &parsed);
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
    forward_native_chat_completions(&state, &target, parsed, &affinity).await
}

fn resolve_kimi_model_to_route(model: &str) -> String {
    if let Some(mapped) = crate::kimi_target::load_alias_route_map().get(model) {
        return mapped.clone();
    }
    model.to_string()
}

/// Forward an already-Chat-Completions body (Kimi agent-gw style) to upstream.
async fn forward_native_chat_completions(
    state: &ProxyState,
    target: &RouteTarget,
    mut request_body: Value,
    affinity: &AffinityInputs,
) -> Response {
    if let Some(object) = request_body.as_object_mut() {
        object.insert("model".into(), Value::String(target.upstream_model.clone()));
    }
    let wants_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
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
        let mut request = state.client.post(&endpoint).json(&request_body);
        let auth = match upstream_auth(state, target, affinity, &exclude).await {
            Ok(Some(auth)) => {
                request = apply_upstream_headers(request, &auth, target);
                Some(auth)
            }
            Ok(None) => {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "no_upstream_credential",
                    "No healthy upstream credential for this route; re-login the account in Codex Spur",
                );
            }
            Err(response) => return response,
        };
        let upstream = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                if let Some(auth) = auth {
                    let _ = state.storage.release_lease(&auth.lease_id).await;
                    if attempt + 1 < max_switches {
                        exclude.push(auth.credential_id);
                        continue;
                    }
                }
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_transport_error",
                    &format!("Upstream request failed: {error}"),
                );
            }
        };
        let status = upstream.status();
        if is_failover_status(status) {
            if let Some(auth) = auth {
                let body_bytes = upstream.bytes().await.unwrap_or_default();
                let headers = Default::default();
                let _ = handle_upstream_failure(
                    state,
                    &target.provider_id,
                    &auth,
                    status,
                    &headers,
                    Some(body_bytes.as_ref()),
                )
                .await;
                let _ = state.storage.release_lease(&auth.lease_id).await;
                if attempt + 1 < max_switches {
                    exclude.push(auth.credential_id);
                    continue;
                }
                return error_response(
                    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                    "upstream_error",
                    &String::from_utf8_lossy(&body_bytes),
                );
            }
        }
        if let Some(auth) = &auth {
            record_diag(
                state,
                target,
                auth,
                attempt,
                "ok",
                false,
                None,
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
        let status_code =
            StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let payload = upstream.json::<Value>().await.unwrap_or_else(|_| {
            json!({"error":{"type":"upstream_error","message":"Upstream request failed"}})
        });
        return (status_code, Json(payload)).into_response();
    }
    error_response(
        StatusCode::BAD_GATEWAY,
        "upstream_retry_exhausted",
        "All eligible accounts failed",
    )
}

async fn kimi_coding_health(State(state): State<ProxyState>) -> Response {
    Json(json!({
        "ok": true,
        "service": "codex-spur-kimi-compat",
        "experimental": true,
        "catalogModels": state.catalog.read().await.models.len(),
    }))
    .into_response()
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
        // Experimental Kimi Desktop agent-gw compatible surface (config injection path).
        .route("/coding/healthz", get(kimi_coding_health))
        .route("/coding/v1/chat/completions", post(kimi_coding_chat_completions))
        .route("/coding/v1/v1/chat/completions", post(kimi_coding_chat_completions_v1))
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
    fn chat_sse_lifecycle_matches_desktop_expectations() {
        // ChatGPT Desktop drops turns that skip created/item/content_part events
        // ("你在 0s 后停止了"). Match Nice Switch / CC Switch envelope.
        let stream = chat_text_to_responses_sse(
            "resp_test1",
            "deepseek-v4-flash",
            1_700_000_000,
            "Hello",
            "",
            Some(&json!({
                "prompt_tokens": 4,
                "completion_tokens": 2,
                "total_tokens": 6
            })),
        );

        let events: Vec<&str> = stream
            .lines()
            .filter_map(|line| line.strip_prefix("event: "))
            .collect();
        assert_eq!(
            events,
            vec![
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ],
            "unexpected event order: {events:?}"
        );
        assert!(
            !stream.contains("data: [DONE]"),
            "Responses SSE must not use Chat [DONE]"
        );
        assert!(stream.contains("\"created_at\":1700000000"));
        assert!(stream.contains("\"input_tokens\":4"));
        assert!(stream.contains("\"delta\":\"Hello\""));
        assert!(stream.contains("\"text\":\"Hello\""));
        // OpenAI requires message item ids to begin with `msg` (not `resp_…_msg`).
        assert!(stream.contains("\"id\":\"msg_test1\""));
        assert!(
            !stream.contains("resp_test1_msg"),
            "legacy suffix form poisons Desktop history for later OpenAI turns"
        );
        // created/in_progress carry empty output; completed carries the message item.
        assert!(stream.contains("\"status\":\"completed\""));
    }

    #[test]
    fn item_ids_use_openai_type_prefixes() {
        // CC Switch style type prefixes: msg_ / rs_ / resp_ (not suffix `_msg`).
        assert_eq!(
            message_item_id_from_response_id("resp_DrEsjCIVmIVVpgzXc7U8pVV4"),
            "msg_DrEsjCIVmIVVpgzXc7U8pVV4"
        );
        assert_eq!(message_item_id_from_response_id("resp_abc_msg"), "msg_abc");
        assert_eq!(
            message_item_id_from_response_id("msg_already"),
            "msg_already"
        );
        assert_eq!(reasoning_item_id_from_response_id("resp_abc"), "rs_abc");
        assert_eq!(response_id_from_chat_id(Some("chatcmpl-xyz")), "resp_xyz");
        assert_eq!(response_id_from_chat_id(Some("chatcmpl_abc")), "resp_abc");
        assert_eq!(response_id_from_chat_id(Some("resp_keep")), "resp_keep");
    }

    #[test]
    fn openai_input_rewrites_legacy_message_ids() {
        let mut request = json!({
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "id": "resp_DrEsjCIVmIVVpgzXc7U8pVV4_msg",
                    "content": [{"type": "output_text", "text": "hi"}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_ok",
                    "content": [{"type": "input_text", "text": "go"}]
                },
                {
                    "type": "function_call",
                    "id": "fc_call1",
                    "call_id": "call1",
                    "name": "shell"
                },
                {
                    "role": "assistant",
                    "id": "25fdab0e-8ec3-413c-8703-7ce5668ec990",
                    "content": "bare uuid"
                }
            ]
        });
        sanitize_openai_input_message_ids(&mut request);
        let input = request["input"].as_array().unwrap();
        assert_eq!(
            input[0]["id"].as_str().unwrap(),
            "msg_DrEsjCIVmIVVpgzXc7U8pVV4"
        );
        assert_eq!(input[1]["id"].as_str().unwrap(), "msg_ok");
        // Non-message items untouched.
        assert_eq!(input[2]["id"].as_str().unwrap(), "fc_call1");
        assert_eq!(
            input[3]["id"].as_str().unwrap(),
            "msg_25fdab0e-8ec3-413c-8703-7ce5668ec990"
        );
    }

    #[test]
    fn openai_sanitize_rewrites_ids_but_keeps_native_tools() {
        let mut request = json!({
            "tools": [{"type": "namespace", "name": "x", "tools": []}],
            "input": [{
                "type": "message",
                "role": "assistant",
                "id": "resp_old_msg",
                "content": []
            }]
        });
        sanitize_responses_request_for_upstream("openai", &mut request);
        // OpenAI kind keeps Codex-native tools (namespace not stripped).
        assert_eq!(request["tools"][0]["type"], "namespace");
        assert_eq!(request["input"][0]["id"], "msg_old");
    }

    #[test]
    fn openai_input_drops_all_reasoning_and_item_refs_when_store_false() {
        // Bridge summary-only reasoning 404s; Grok encrypted_content decrypts fail.
        // OpenAI path drops every reasoning item so Grok/DeepSeek → GPT is safe.
        let mut request = json!({
            "store": false,
            "previous_response_id": "resp_DrEsjCIVmIVVpgzXc7U8pVV4",
            "input": [
                {
                    "type": "reasoning",
                    "id": "rs_resp_DrEsjCIVmIVVpgzXc7U8pVV4",
                    "summary": [{"type": "summary_text", "text": "plan"}]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": "resp_DrEsjCIVmIVVpgzXc7U8pVV4_msg",
                    "content": [{"type": "output_text", "text": "ok"}]
                },
                {
                    "type": "item_reference",
                    "id": "rs_resp_DrEsjCIVmIVVpgzXc7U8pVV4"
                },
                {
                    "type": "reasoning",
                    "id": "rs_grok_foreign",
                    "encrypted_content": "opaque-ciphertext-from-xai",
                    "summary": []
                },
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_user1",
                    "content": [{"type": "input_text", "text": "hi"}]
                }
            ]
        });
        sanitize_openai_responses_input(&mut request);
        let input = request["input"].as_array().unwrap();
        assert_eq!(
            input.len(),
            2,
            "all reasoning + item_ref dropped: {input:?}"
        );
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["id"], "msg_DrEsjCIVmIVVpgzXc7U8pVV4");
        assert_eq!(input[1]["id"], "msg_user1");
        assert!(input.iter().all(|item| item["type"] != "reasoning"));
        // Dropped items → strip sticky previous_response_id.
        assert!(request.get("previous_response_id").is_none());
    }

    #[test]
    fn openai_input_drops_encrypted_grok_reasoning_even_when_non_empty() {
        let mut request = json!({
            "store": false,
            "previous_response_id": "resp_sticky",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_user1",
                    "content": [{"type": "input_text", "text": "hi"}]
                },
                {
                    "type": "reasoning",
                    "id": "rs_grok",
                    "encrypted_content": "cipher",
                    "summary": []
                }
            ]
        });
        sanitize_openai_responses_input(&mut request);
        let input = request["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["id"], "msg_user1");
        assert!(request.get("previous_response_id").is_none());
    }

    #[test]
    fn openai_input_keeps_previous_response_id_when_nothing_dropped() {
        let mut request = json!({
            "store": false,
            "previous_response_id": "resp_openai_native",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_user1",
                    "content": [{"type": "input_text", "text": "hi"}]
                },
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"ls\"}"
                }
            ]
        });
        sanitize_openai_responses_input(&mut request);
        assert_eq!(request["previous_response_id"], "resp_openai_native");
        assert_eq!(request["input"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn openai_input_store_true_keeps_item_reference_but_drops_empty_reasoning() {
        let mut request = json!({
            "store": true,
            "previous_response_id": "resp_keep",
            "input": [
                {
                    "type": "item_reference",
                    "id": "msg_server_side"
                },
                {
                    "type": "reasoning",
                    "id": "rs_resp_bridge",
                    "summary": [{"type": "summary_text", "text": "x"}]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": "resp_x_msg",
                    "content": []
                }
            ]
        });
        sanitize_openai_responses_input(&mut request);
        let input = request["input"].as_array().unwrap();
        // store=true: item_reference kept; bridge reasoning still dropped.
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "item_reference");
        assert_eq!(input[1]["id"], "msg_x");
        assert!(request.get("previous_response_id").is_none());
    }

    #[test]
    fn chat_sse_includes_reasoning_before_message() {
        let stream = chat_text_to_responses_sse(
            "resp_r1",
            "deepseek-v4-flash",
            42,
            "pong",
            "Need context.",
            None,
        );
        let events: Vec<&str> = stream
            .lines()
            .filter_map(|line| line.strip_prefix("event: "))
            .collect();
        assert!(events.contains(&"response.reasoning_summary_text.delta"));
        let reasoning_pos = stream
            .find("\"type\":\"reasoning\"")
            .expect("reasoning item");
        let message_pos = stream.find("\"type\":\"message\"").expect("message item");
        assert!(
            reasoning_pos < message_pos,
            "reasoning must precede assistant message"
        );
        assert!(stream.contains("Need context."));
        assert!(stream.contains("\"text\":\"pong\""));
        assert!(stream.contains("\"id\":\"rs_r1\""));
        assert!(stream.contains("\"id\":\"msg_r1\""));
    }

    #[test]
    fn parse_chat_sse_collects_text_reasoning_and_usage() {
        let body = r#"
data: {"id":"chatcmpl_abc","created":99,"choices":[{"delta":{"reasoning_content":"think "}}]}

data: {"id":"chatcmpl_abc","created":99,"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}

data: [DONE]
"#;
        let parsed = parse_chat_completions_sse(body);
        assert_eq!(parsed.response_id, "resp_abc");
        assert_eq!(parsed.created_at, 99);
        assert_eq!(parsed.text, "hi");
        assert_eq!(parsed.reasoning, "think ");
        assert!(parsed.tool_calls.is_empty());
        assert_eq!(parsed.usage.as_ref().unwrap()["prompt_tokens"], 3);
    }

    #[test]
    fn parse_chat_sse_assembles_streaming_tool_calls() {
        // Repro: DeepSeek thinks then calls exec_command — must not drop tool_calls.
        let body = r#"
data: {"id":"chatcmpl_tools","created":1,"choices":[{"delta":{"reasoning_content":"Need to read the file."}}]}

data: {"id":"chatcmpl_tools","created":1,"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"exec_command"}}]}}]}

data: {"id":"chatcmpl_tools","created":1,"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"cat card-game.html\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}

data: [DONE]
"#;
        let parsed = parse_chat_completions_sse(body);
        assert_eq!(parsed.reasoning, "Need to read the file.");
        assert!(parsed.text.is_empty());
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "call_1");
        assert_eq!(parsed.tool_calls[0].name, "exec_command");
        assert_eq!(
            parsed.tool_calls[0].arguments,
            "{\"cmd\":\"cat card-game.html\"}"
        );

        let stream = chat_parsed_to_responses_sse(
            &parsed.response_id,
            "deepseek-v4-flash",
            parsed.created_at,
            &parsed.text,
            &parsed.reasoning,
            &parsed.tool_calls,
            parsed.usage.as_ref(),
        );
        assert!(stream.contains("\"type\":\"function_call\""));
        assert!(stream.contains("event: response.function_call_arguments.delta"));
        assert!(stream.contains("event: response.function_call_arguments.done"));
        assert!(stream.contains("exec_command"));
        assert!(stream.contains("cat card-game.html"));
        // Tool-only turn: no empty assistant message required.
        // Reasoning should still appear before the tool call.
        let reasoning_pos = stream.find("\"type\":\"reasoning\"").expect("reasoning");
        let fc_pos = stream.find("\"type\":\"function_call\"").expect("function_call");
        assert!(reasoning_pos < fc_pos);
    }

    #[test]
    fn chat_history_preserves_function_call_and_output() {
        // Grok wrote the file via function_call; DeepSeek must see tool trail.
        // message → function_call → output merges into one assistant turn so
        // tool_calls are immediately followed by tool results.
        let messages = response_input_to_messages(Some(&json!([
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "写卡牌"}]
            },
            {
                "type": "reasoning",
                "id": "rs_skip",
                "encrypted_content": "foreign",
                "summary": [{"type": "summary_text", "text": "plan"}]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "先写文件"}]
            },
            {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call-write-1",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"cat > game.html << 'EOF'\\n<html/>\\nEOF\"}"
            },
            {
                "type": "function_call_output",
                "id": "fco_1",
                "call_id": "call-write-1",
                "output": "done"
            },
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "增加二种卡牌"}]
            }
        ])));
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "先写文件");
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call-write-1");
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            "exec_command"
        );
        assert!(messages[1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains("game.html"));
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call-write-1");
        assert_eq!(messages[2]["content"], "done");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[3]["content"], "增加二种卡牌");
        // Foreign reasoning must not become a chat message.
        assert!(messages.iter().all(|m| m["role"] != "reasoning"));
    }

    #[test]
    fn chat_history_merges_function_call_before_assistant_text() {
        // Repro: session 019f8528 Kimi turn — chat-bridge SSE emits tools then
        // text, Desktop stores function_call → message → function_call_output.
        // Old converter flushed tool_calls, then a second assistant text, which
        // Kimi rejected: "tool_call_ids did not have response messages: exec_command:N".
        let messages = response_input_to_messages(Some(&json!([
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "开始执行"}]
            },
            {
                "type": "function_call",
                "id": "fc_j2KYL0aT8fnUDkIBpgFCqbx7_0",
                "call_id": "tool_Y58P6C3PYwSwKzXPpd1gMJ3g",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"lsof -iTCP -sTCP:LISTEN\"}"
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Kimi 正在运行。我先做安全备份，再用两种方法抓它的网络流量：本地代理 + lsof/pfctl。"
                }]
            },
            {
                "type": "function_call_output",
                "call_id": "tool_Y58P6C3PYwSwKzXPpd1gMJ3g",
                "output": "ok"
            }
        ])));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("Kimi 正在运行"),
            "assistant text must ride on the same message as tool_calls"
        );
        assert_eq!(
            messages[1]["tool_calls"][0]["id"],
            "tool_Y58P6C3PYwSwKzXPpd1gMJ3g"
        );
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(
            messages[2]["tool_call_id"],
            "tool_Y58P6C3PYwSwKzXPpd1gMJ3g"
        );
        // No intervening assistant-only message between tool_calls and tool result.
        assert!(
            messages
                .windows(2)
                .all(|w| !(w[0].get("tool_calls").is_some() && w[1]["role"] == "assistant")),
            "tool_calls assistant must not be followed by another assistant"
        );
    }

    #[test]
    fn reasoning_only_without_tools_still_emits_message_item() {
        let stream = chat_parsed_to_responses_sse(
            "resp_empty",
            "deepseek-v4-flash",
            1,
            "",
            "I should check the file first.",
            &[],
            None,
        );
        // Avoid the silent "thinking then shut down" shape: always emit a message
        // when there are no tools, even if text is empty.
        assert!(stream.contains("\"type\":\"reasoning\""));
        assert!(stream.contains("\"type\":\"message\""));
        assert!(stream.contains("response.completed"));
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
        assert_eq!(responses_role_to_chat_role("developer"), "system");
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
                "type": "namespace",
                "name": "browser",
                "tools": [{
                    "type": "function",
                    "name": "open_url",
                    "parameters": {"type": "object", "properties": {}}
                }]
            },
            // Named namespace must NOT become a fake function tool.
            {"type": "namespace", "name": "codex"},
            {
                "type": "function",
                "function": {
                    "name": "already_chat",
                    "parameters": {"type": "object", "properties": {}}
                }
            }
        ])));
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["function"]["name"], "get_weather");
        assert_eq!(tools[1]["function"]["name"], "open_url");
        assert_eq!(tools[2]["function"]["name"], "already_chat");
        assert!(tools.iter().all(|t| t["type"] == "function"));
        assert!(tools.iter().all(|t| t["function"]["name"] != "codex"));
    }

    fn sample_codex_tools() -> Value {
        json!([
            {"type": "namespace", "name": "codex"},
            {
                "type": "namespace",
                "name": "browser",
                "tools": [
                    {
                        "type": "function",
                        "name": "open_url",
                        "parameters": {"type": "object", "properties": {}}
                    }
                ]
            },
            {"type": "local_shell"},
            {
                "type": "function",
                "name": "get_weather",
                "parameters": {"type": "object", "properties": {}}
            },
            {"type": "custom", "name": "apply_patch"}
        ])
    }

    #[test]
    fn xai_responses_tools_drop_namespace_and_remap_local_shell() {
        let mut body = json!({
            "model": "grok-4.5",
            "tools": sample_codex_tools()
        });
        sanitize_responses_request_for_upstream("xai", &mut body);
        let tools = body["tools"].as_array().expect("tools kept");
        let types: Vec<&str> = tools
            .iter()
            .map(|tool| tool["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, vec!["function", "shell", "function"]);
        assert_eq!(tools[0]["name"], "open_url");
        assert_eq!(tools[2]["name"], "get_weather");
        assert!(!types.contains(&"namespace"));
        assert!(!types.contains(&"local_shell"));
        assert!(!types.contains(&"custom"));
    }

    #[test]
    fn non_openai_responses_kinds_all_sanitize_codex_tools() {
        for kind in ["xai", "minimax", "custom", "kimi", "deepseek"] {
            let mut body = json!({"tools": sample_codex_tools()});
            sanitize_responses_request_for_upstream(kind, &mut body);
            let tools = body
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                tools.iter().all(|t| {
                    let ty = t["type"].as_str().unwrap_or("");
                    ty != "namespace" && ty != "local_shell" && ty != "custom"
                }),
                "{kind} must not forward Codex-only tool types"
            );
            assert!(
                tools
                    .iter()
                    .any(|t| t["name"] == "open_url" || t["function"]["name"] == "open_url"),
                "{kind} should flatten nested function tools from namespace"
            );
            assert!(
                tools
                    .iter()
                    .any(|t| t["name"] == "get_weather" || t["function"]["name"] == "get_weather"),
                "{kind} should keep freeform function tools"
            );
        }
    }

    #[test]
    fn openai_entry_methods_passthrough_namespace() {
        // kind=openai covers 官方订阅 / JSON 多账号 / API Key — no stripping.
        let mut body = json!({
            "tools": [
                {"type": "namespace", "name": "codex"},
                {"type": "local_shell"}
            ]
        });
        sanitize_responses_request_for_upstream("openai", &mut body);
        let tools = body["tools"].as_array().expect("unchanged");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "namespace");
        assert_eq!(tools[1]["type"], "local_shell");
    }

    #[test]
    fn xai_responses_tools_omit_key_when_all_unsupported() {
        let mut body = json!({
            "tools": [
                {"type": "namespace", "name": "codex"},
                {"type": "custom"}
            ]
        });
        sanitize_responses_request_for_upstream("xai", &mut body);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn xai_drops_namespace_tool_choice_and_additional_tools_input() {
        let mut body = json!({
            "model": "grok-4.5",
            "tools": [
                {"type": "namespace", "name": "client_tools"},
                {"type": "function", "name": "lookup", "parameters": {"type": "object"}}
            ],
            "tool_choice": {"type": "namespace", "name": "client_tools"},
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "additional_tools", "tools": []},
                {"type": "reasoning", "content": null, "summary": []}
            ],
            "prompt_cache_retention": "24h",
            "safety_identifier": "user-1",
            "presence_penalty": 0.5,
            "external_web_access": true
        });
        sanitize_responses_request_for_upstream("xai", &mut body);
        assert!(
            body.get("tool_choice").is_none(),
            "namespace tool_choice must be dropped"
        );
        assert!(body.get("prompt_cache_retention").is_none());
        assert!(body.get("safety_identifier").is_none());
        assert!(body.get("presence_penalty").is_none());
        assert!(body.get("external_web_access").is_none());
        let input = body["input"].as_array().expect("input");
        assert!(
            input
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("additional_tools")),
            "additional_tools input rows must be dropped"
        );
        assert!(
            input
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("reasoning")),
            "all reasoning (incl. content:null) must be dropped on non-OpenAI path"
        );
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        let tools = body["tools"].as_array().expect("function tool kept");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "lookup");
    }

    /// Accident replay: official GPT-5.4-Mini → Grok mid-thread (thread 019f8111…).
    /// OpenAI `encrypted_content` (gAAAAA…) must never reach xAI Responses.
    #[test]
    fn xai_drops_openai_encrypted_reasoning_and_sticky_response_id() {
        let mut body = json!({
            "model": "grok-4.5",
            "store": false,
            "previous_response_id": "resp_openai_from_gpt_5_4_mini",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_user1",
                    "content": [{"type": "input_text", "text": "帮我增加两种卡牌"}]
                },
                {
                    "type": "reasoning",
                    "id": "rs_047fc67e6232ba7c016a5e7b8dc31081919ccb5a219f3b634c",
                    "summary": [],
                    "encrypted_content": "gAAAAABqXnuOf4uNJwdYNDcUaog-J2H50PXEqDM9foreign-openai-cipher"
                },
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"echo ok\"}",
                    "call_id": "call_EQYnOdluvZTEKyRshRkIfNts"
                },
                {
                    "type": "function_call_output",
                    "id": "fc_out_1",
                    "call_id": "call_EQYnOdluvZTEKyRshRkIfNts",
                    "output": "ok"
                },
                {
                    "type": "reasoning",
                    "id": "rs_047fc67e6232ba7c016a5e7bc38bbc8191ad230980d9814db7",
                    "summary": [],
                    "encrypted_content": "gAAAAABqXnvDQ6aXvKUNq30e3Hf-5MSbVzhcFz4Qanother-openai-blob"
                },
                {
                    "type": "item_reference",
                    "id": "rs_047fc67e6232ba7c016a5e7b8dc31081919ccb5a219f3b634c"
                },
                {
                    "type": "message",
                    "role": "user",
                    "id": "msg_continue",
                    "content": [{"type": "input_text", "text": "继续"}]
                }
            ]
        });
        sanitize_responses_request_for_upstream("xai", &mut body);
        let input = body["input"].as_array().expect("input");
        assert!(
            input
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("reasoning")),
            "OpenAI encrypted reasoning must not reach Grok: {input:?}"
        );
        assert!(
            input
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("item_reference")),
            "item_reference must be dropped under store=false"
        );
        // Tool history + messages kept so Grok can continue the card-game work.
        assert!(input.iter().any(|i| i.get("type") == Some(&json!("function_call"))));
        assert!(input
            .iter()
            .any(|i| i.get("type") == Some(&json!("function_call_output"))));
        assert!(input.iter().any(|i| {
            i.get("type") == Some(&json!("message"))
                && i.get("id") == Some(&json!("msg_continue"))
        }));
        assert!(
            body.get("previous_response_id").is_none(),
            "foreign previous_response_id must be stripped after drops"
        );
        // No encrypted_content anywhere in the outbound body.
        let serialized = body.to_string();
        assert!(
            !serialized.contains("encrypted_content"),
            "ciphertext must not appear in xAI request"
        );
        assert!(!serialized.contains("gAAAAA"));
    }

    #[test]
    fn openai_responses_keeps_codex_input_and_tool_choice() {
        let mut body = json!({
            "tools": [{"type": "namespace", "name": "codex"}],
            "tool_choice": {"type": "namespace", "name": "codex"},
            "input": [{"type": "additional_tools", "tools": []}],
            "prompt_cache_retention": "24h"
        });
        sanitize_responses_request_for_upstream("openai", &mut body);
        assert_eq!(body["tools"][0]["type"], "namespace");
        assert_eq!(body["tool_choice"]["type"], "namespace");
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert_eq!(body["prompt_cache_retention"], "24h");
    }

    #[test]
    fn summarize_upstream_error_body_reads_message() {
        let body = br#"{"error":{"message":"unknown variant `namespace`","type":"invalid_request_error"}}"#;
        let summary = summarize_upstream_error_body(body).expect("summary");
        assert!(summary.contains("namespace"));
        assert!(summary.len() <= 200);
    }

    #[test]
    fn chat_path_drops_invalid_tool_choice() {
        let chat = responses_to_chat_completions(
            &json!({
                "model": "ignored",
                "input": "hi",
                "tools": [{"type": "namespace", "name": "codex"}],
                "tool_choice": {"type": "namespace", "name": "codex"}
            }),
            "deepseek-chat",
        );
        assert!(chat.get("tools").is_none() || chat["tools"].as_array().unwrap().is_empty());
        assert!(chat.get("tool_choice").is_none());
    }

    #[test]
    fn chat_path_flattens_namespace_and_drops_shell() {
        let chat = responses_to_chat_completions(
            &json!({
                "model": "ignored",
                "input": "hi",
                "tools": sample_codex_tools()
            }),
            "kimi-for-coding",
        );
        let tools = chat["tools"].as_array().expect("function tools kept");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["function"]["name"], "open_url");
        assert_eq!(tools[1]["function"]["name"], "get_weather");
        assert!(tools.iter().all(|t| t["type"] == "function"));
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
        assert!(
            chat.get("tools").is_none(),
            "unsupported tools must be dropped"
        );
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
