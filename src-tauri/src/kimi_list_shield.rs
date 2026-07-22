//! Local HTTP CONNECT proxy that blocks Kimi Work model-list cloud host.
//!
//! Purpose: after "publish to Kimi", force `DescribeKimiWorkConfig` to fail so the
//! app falls back to `kimi-work-models-cache.json` (with Spur models).
//!
//! Without TLS MITM we cannot match a single URL path. This shield therefore
//! blocks the whole host `www.kimi.com` (where ConfigService lives) and tunnels
//! everything else — including `agent-gw.kimi.com`.
//!
//! Traffic only reaches this proxy if the OS/app is configured to use it
//! (system HTTPS proxy or Proxyman → this port). Bind: 127.0.0.1 only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};

const DEFAULT_PORT: u16 = 17862;
const BLOCKED_HOSTS: &[&str] = &["www.kimi.com"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiListShieldStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub listen: Option<String>,
    pub blocked_hosts: Vec<String>,
    pub blocked_connects: u64,
    pub tunneled_connects: u64,
    pub note: String,
}

struct ShieldMetrics {
    blocked: AtomicU64,
    tunneled: AtomicU64,
}

struct ShieldInner {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    metrics: Arc<ShieldMetrics>,
}

#[derive(Default)]
pub struct KimiListShield {
    inner: Mutex<Option<ShieldInner>>,
}

impl KimiListShield {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub async fn status(&self) -> KimiListShieldStatus {
        let guard = self.inner.lock().await;
        match guard.as_ref() {
            Some(s) => KimiListShieldStatus {
                running: true,
                port: Some(s.port),
                listen: Some(format!("127.0.0.1:{}", s.port)),
                blocked_hosts: BLOCKED_HOSTS.iter().map(|s| (*s).to_string()).collect(),
                blocked_connects: s.metrics.blocked.load(Ordering::Relaxed),
                tunneled_connects: s.metrics.tunneled.load(Ordering::Relaxed),
                note: "系统 HTTPS 代理需指向此地址后，Kimi 才会走拦截。勿拦 agent-gw（已自动放行）。".into(),
            },
            None => KimiListShieldStatus {
                running: false,
                port: None,
                listen: None,
                blocked_hosts: BLOCKED_HOSTS.iter().map(|s| (*s).to_string()).collect(),
                blocked_connects: 0,
                tunneled_connects: 0,
                note: "列表保护未运行。发布到 Kimi 后可一键开启。".into(),
            },
        }
    }

    pub async fn start(&self) -> Result<KimiListShieldStatus> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(KimiListShieldStatus {
                running: true,
                port: Some(existing.port),
                listen: Some(format!("127.0.0.1:{}", existing.port)),
                blocked_hosts: BLOCKED_HOSTS.iter().map(|s| (*s).to_string()).collect(),
                blocked_connects: existing.metrics.blocked.load(Ordering::Relaxed),
                tunneled_connects: existing.metrics.tunneled.load(Ordering::Relaxed),
                note: "列表保护已在运行。".into(),
            });
        }

        let (listener, port) = bind_shield_listener().await?;
        let metrics = Arc::new(ShieldMetrics {
            blocked: AtomicU64::new(0),
            tunneled: AtomicU64::new(0),
        });
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let metrics_task = Arc::clone(&metrics);

        tokio::spawn(async move {
            run_accept_loop(listener, metrics_task, shutdown_rx).await;
        });

        *guard = Some(ShieldInner {
            port,
            shutdown: Some(shutdown_tx),
            metrics: Arc::clone(&metrics),
        });

        tracing::info!(port, "kimi list shield started on 127.0.0.1");
        Ok(KimiListShieldStatus {
            running: true,
            port: Some(port),
            listen: Some(format!("127.0.0.1:{port}")),
            blocked_hosts: BLOCKED_HOSTS.iter().map(|s| (*s).to_string()).collect(),
            blocked_connects: 0,
            tunneled_connects: 0,
            note: format!(
                "已监听 127.0.0.1:{port}。请将系统 HTTPS 代理设为该地址，然后完全退出并重开 Kimi。"
            ),
        })
    }

    pub async fn stop(&self) -> Result<KimiListShieldStatus> {
        let mut guard = self.inner.lock().await;
        if let Some(mut inner) = guard.take() {
            if let Some(tx) = inner.shutdown.take() {
                let _ = tx.send(());
            }
            tracing::info!(port = inner.port, "kimi list shield stopped");
        }
        Ok(self.status_unlocked_stopped())
    }

    fn status_unlocked_stopped(&self) -> KimiListShieldStatus {
        KimiListShieldStatus {
            running: false,
            port: None,
            listen: None,
            blocked_hosts: BLOCKED_HOSTS.iter().map(|s| (*s).to_string()).collect(),
            blocked_connects: 0,
            tunneled_connects: 0,
            note: "列表保护已停止。".into(),
        }
    }
}

async fn bind_shield_listener() -> Result<(TcpListener, u16)> {
    let mut port = DEFAULT_PORT;
    loop {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(err) if port < DEFAULT_PORT + 32 => {
                tracing::warn!(port, %err, "kimi list shield port busy, trying next");
                port += 1;
            }
            Err(err) => {
                return Err(anyhow!("无法绑定列表保护端口 {DEFAULT_PORT}+：{err}"));
            }
        }
    }
}

async fn run_accept_loop(
    listener: TcpListener,
    metrics: Arc<ShieldMetrics>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let metrics = Arc::clone(&metrics);
                        tokio::spawn(async move {
                            if let Err(err) = handle_client(stream, metrics).await {
                                tracing::debug!(%err, "kimi list shield client ended");
                            }
                        });
                    }
                    Err(err) => {
                        tracing::warn!(%err, "kimi list shield accept failed");
                    }
                }
            }
        }
    }
}

fn host_is_blocked(host: &str) -> bool {
    let host = host.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    // Never block localhost / spur / agent-gw
    if host == "127.0.0.1"
        || host == "localhost"
        || host == "::1"
        || host.ends_with(".localhost")
        || host == "agent-gw.kimi.com"
        || host.ends_with(".agent-gw.kimi.com")
    {
        return false;
    }
    BLOCKED_HOSTS
        .iter()
        .any(|b| host == *b || host.ends_with(&format!(".{b}")))
}

async fn handle_client(mut client: TcpStream, metrics: Arc<ShieldMetrics>) -> Result<()> {
    let header = read_headers(&mut client).await?;
    let (method, target) = parse_request_line(&header)?;

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(&target, 443)?;
        if host_is_blocked(&host) {
            metrics.blocked.fetch_add(1, Ordering::Relaxed);
            let _ = client
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\n\
                      Connection: close\r\n\
                      Content-Type: text/plain; charset=utf-8\r\n\
                      \r\n\
                      blocked by Codex Spur Kimi list shield (www.kimi.com)\n",
                )
                .await;
            return Ok(());
        }
        let mut upstream = TcpStream::connect((host.as_str(), port))
            .await
            .with_context(|| format!("CONNECT upstream {host}:{port}"))?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        metrics.tunneled.fetch_add(1, Ordering::Relaxed);
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
        return Ok(());
    }

    // Absolute-form HTTP proxy request: GET http://host/path
    if let Some(host) = absolute_url_host(&target) {
        if host_is_blocked(&host) {
            metrics.blocked.fetch_add(1, Ordering::Relaxed);
            let _ = client
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\n\
                      Connection: close\r\n\
                      Content-Type: text/plain; charset=utf-8\r\n\
                      \r\n\
                      blocked by Codex Spur Kimi list shield\n",
                )
                .await;
            return Ok(());
        }
    }

    // Best-effort: refuse non-CONNECT without full re-issue (Kimi uses HTTPS CONNECT).
    bail!("unsupported proxy method {method} (expected CONNECT for HTTPS)");
}

async fn read_headers(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 256];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            bail!("client closed before headers complete");
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            bail!("proxy headers too large");
        }
    }
    Ok(buf)
}

fn parse_request_line(headers: &[u8]) -> Result<(String, String)> {
    let text = String::from_utf8_lossy(headers);
    let line = text.lines().next().unwrap_or("").trim();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    if method.is_empty() || target.is_empty() {
        bail!("bad request line");
    }
    Ok((method, target))
}

fn split_host_port(target: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(rest) = target.strip_prefix('[') {
        // [ipv6]:port
        if let Some((host, port)) = rest.split_once("]:") {
            let port: u16 = port.parse().unwrap_or(default_port);
            return Ok((host.to_string(), port));
        }
    }
    if let Some((host, port)) = target.rsplit_once(':') {
        if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            let port: u16 = port.parse().unwrap_or(default_port);
            return Ok((host.to_string(), port));
        }
    }
    Ok((target.to_string(), default_port))
}

fn absolute_url_host(target: &str) -> Option<String> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_www_kimi_not_agent_gw() {
        assert!(host_is_blocked("www.kimi.com"));
        assert!(host_is_blocked("WWW.Kimi.COM"));
        assert!(!host_is_blocked("agent-gw.kimi.com"));
        assert!(!host_is_blocked("127.0.0.1"));
        assert!(!host_is_blocked("api.example.com"));
    }

    #[test]
    fn split_host_port_works() {
        assert_eq!(
            split_host_port("www.kimi.com:443", 443).unwrap(),
            ("www.kimi.com".into(), 443)
        );
    }
}
