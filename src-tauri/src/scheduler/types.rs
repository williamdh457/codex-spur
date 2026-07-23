use serde::{Deserialize, Serialize};

/// Instance-level routing: pool scheduling or a fixed credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RoutingMode {
    Pool,
    Fixed,
}

impl RoutingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pool => "pool",
            Self::Fixed => "fixed",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "fixed" => Self::Fixed,
            _ => Self::Pool,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionLayer {
    Fixed,
    PreviousResponse,
    Session,
    LoadBalance,
}

impl SelectionLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::PreviousResponse => "previous_response",
            Self::Session => "session",
            Self::LoadBalance => "load_balance",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    PreviousResponse,
    Session,
}

impl BindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreviousResponse => "previous_response",
            Self::Session => "session",
        }
    }

    #[allow(dead_code)]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "previous_response" => Some(Self::PreviousResponse),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    Ready,
    AuthInvalid,
    Entitlement,
    RateLimited,
}

impl ScheduleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::AuthInvalid => "auth_invalid",
            Self::Entitlement => "entitlement",
            Self::RateLimited => "rate_limited",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "auth_invalid" => Self::AuthInvalid,
            "entitlement" => Self::Entitlement,
            "rate_limited" => Self::RateLimited,
            _ => Self::Ready,
        }
    }

    pub fn is_schedulable(self) -> bool {
        matches!(self, Self::Ready | Self::RateLimited)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScoreWeights {
    pub priority: f64,
    pub load: f64,
    pub queue: f64,
    pub error_rate: f64,
    pub ttft: f64,
    /// Prefer accounts whose session window resets soonest (use-it-or-lose-it).
    pub reset: f64,
    pub quota_headroom: f64,
    /// Prefer lower upstream billing multiplier (Sub2API-like; default off).
    #[serde(default)]
    pub upstream_cost: f64,
    /// Soft sticky bonus for previous_response_id binding (sticky-weighted mode only).
    #[serde(default = "default_weight_previous_response")]
    pub previous_response: f64,
    /// Soft sticky bonus for session-hash binding (sticky-weighted mode only).
    #[serde(default = "default_weight_session_sticky")]
    pub session_sticky: f64,
}

fn default_weight_previous_response() -> f64 {
    5.0
}

fn default_weight_session_sticky() -> f64 {
    3.0
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            priority: 1.0,
            load: 1.0,
            queue: 0.7,
            error_rate: 0.8,
            ttft: 0.5,
            reset: 0.0,
            // Prefer accounts with remaining quota when snapshots are fresh (Spur desktop default).
            quota_headroom: 1.0,
            upstream_cost: 0.0,
            previous_response: default_weight_previous_response(),
            session_sticky: default_weight_session_sticky(),
        }
    }
}

/// Fallback account selection after a concurrency wait (Sub2API-like).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FallbackSelectionMode {
    #[default]
    LastUsed,
    Random,
}

impl FallbackSelectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastUsed => "last_used",
            Self::Random => "random",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "random" => Self::Random,
            _ => Self::LastUsed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StickyEscapeConfig {
    pub enabled: bool,
    pub ttft_ms: f64,
    pub error_rate: f64,
}

impl Default for StickyEscapeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttft_ms: 15_000.0,
            error_rate: 0.5,
        }
    }
}

/// Pool-level scheduler knobs (Sub2API-like defaults).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PoolSchedulerConfig {
    pub lb_top_k: u32,
    pub sticky_session_ttl_secs: i64,
    pub sticky_response_id_ttl_secs: i64,
    pub score_weights: ScoreWeights,
    pub sticky_escape: StickyEscapeConfig,
    pub prefer_soonest_reset: bool,
    pub default_429_cooldown_secs: i64,
    pub max_failover_switches: u32,
    pub lease_ttl_secs: i64,
    /// When a fresh quota snapshot shows remaining ≤ this fraction, skip the account.
    #[serde(default = "default_true")]
    pub exclude_zero_quota: bool,
    /// Exclude when 5h used_percent/100 ≥ threshold (0 disables). Default 1.0 = only fully exhausted.
    #[serde(default = "default_quota_pause_threshold")]
    pub quota_auto_pause_5h: f64,
    /// Exclude when 7d used fraction ≥ threshold (0 disables).
    #[serde(default = "default_quota_pause_threshold")]
    pub quota_auto_pause_7d: f64,
    /// Prefer waiting for a sticky account's concurrency slot instead of switching.
    #[serde(default = "default_true")]
    pub sticky_wait_enabled: bool,
    /// Max seconds to wait for sticky concurrency (Sub2API default 120s).
    #[serde(default = "default_sticky_wait_timeout")]
    pub sticky_wait_timeout_secs: i64,
    /// Max concurrent waiters for one sticky account (Sub2API sticky_session_max_waiting=3).
    #[serde(default = "default_sticky_wait_max_waiting")]
    pub sticky_wait_max_waiting: u32,
    /// When every eligible account is concurrency-full, wait for any slot.
    #[serde(default = "default_true")]
    pub fallback_wait_enabled: bool,
    /// Max seconds for fallback concurrency wait (Sub2API 30s).
    #[serde(default = "default_fallback_wait_timeout")]
    pub fallback_wait_timeout_secs: i64,
    /// Max concurrent fallback waiters (Sub2API 100).
    #[serde(default = "default_fallback_max_waiting")]
    pub fallback_max_waiting: u32,
    /// How to pick after fallback wait (Sub2API last_used | random).
    #[serde(default)]
    pub fallback_selection_mode: FallbackSelectionMode,
    /// Soft sticky: affinity becomes score bonuses instead of hard hits (default off).
    #[serde(default)]
    pub sticky_weighted_enabled: bool,
    /// Apply cooldown after 429 / usage_limit (default on).
    #[serde(default = "default_true")]
    pub rate_limit_429_cooldown_enabled: bool,
    /// Cooldown after 529 overloaded (Sub2API default 10 minutes).
    #[serde(default = "default_overload_529_cooldown")]
    pub overload_529_cooldown_secs: i64,
    /// Allow failover on selected 400 errors (default off; Sub2API failover_on_400).
    #[serde(default)]
    pub failover_on_400: bool,
}

fn default_true() -> bool {
    true
}

fn default_quota_pause_threshold() -> f64 {
    1.0
}

fn default_sticky_wait_timeout() -> i64 {
    120
}

fn default_sticky_wait_max_waiting() -> u32 {
    3
}

fn default_fallback_wait_timeout() -> i64 {
    30
}

fn default_fallback_max_waiting() -> u32 {
    100
}

fn default_overload_529_cooldown() -> i64 {
    600
}

impl Default for PoolSchedulerConfig {
    fn default() -> Self {
        Self {
            lb_top_k: 7,
            sticky_session_ttl_secs: 3600,
            sticky_response_id_ttl_secs: 3600,
            score_weights: ScoreWeights::default(),
            sticky_escape: StickyEscapeConfig::default(),
            prefer_soonest_reset: false,
            default_429_cooldown_secs: 30,
            max_failover_switches: 10,
            lease_ttl_secs: 900,
            exclude_zero_quota: true,
            quota_auto_pause_5h: 1.0,
            quota_auto_pause_7d: 1.0,
            sticky_wait_enabled: true,
            sticky_wait_timeout_secs: 120,
            sticky_wait_max_waiting: 3,
            fallback_wait_enabled: true,
            fallback_wait_timeout_secs: 30,
            fallback_max_waiting: 100,
            fallback_selection_mode: FallbackSelectionMode::LastUsed,
            sticky_weighted_enabled: false,
            rate_limit_429_cooldown_enabled: true,
            overload_529_cooldown_secs: 600,
            failover_on_400: false,
        }
    }
}

impl PoolSchedulerConfig {
    pub fn from_json(raw: &str) -> Self {
        if raw.trim().is_empty() || raw.trim() == "{}" {
            return Self::default();
        }
        match serde_json::from_str::<PoolSchedulerConfig>(raw) {
            Ok(mut cfg) => {
                cfg.sanitize();
                cfg
            }
            Err(_) => Self::default(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn sanitize(&mut self) {
        if self.lb_top_k == 0 {
            self.lb_top_k = 1;
        }
        if self.lb_top_k > 64 {
            self.lb_top_k = 64;
        }
        if self.sticky_session_ttl_secs < 60 {
            self.sticky_session_ttl_secs = 60;
        }
        if self.sticky_response_id_ttl_secs < 60 {
            self.sticky_response_id_ttl_secs = 60;
        }
        if self.default_429_cooldown_secs < 1 {
            self.default_429_cooldown_secs = 1;
        }
        if self.max_failover_switches == 0 {
            self.max_failover_switches = 1;
        }
        if self.max_failover_switches > 20 {
            self.max_failover_switches = 20;
        }
        if self.lease_ttl_secs < 60 {
            self.lease_ttl_secs = 60;
        }
        // Sub2API sticky wait is 120s; allow up to 300s for power users.
        self.sticky_wait_timeout_secs = self.sticky_wait_timeout_secs.clamp(0, 300);
        if self.sticky_wait_max_waiting == 0 {
            self.sticky_wait_max_waiting = 1;
        }
        if self.sticky_wait_max_waiting > 64 {
            self.sticky_wait_max_waiting = 64;
        }
        self.fallback_wait_timeout_secs = self.fallback_wait_timeout_secs.clamp(0, 300);
        if self.fallback_max_waiting == 0 {
            self.fallback_max_waiting = 1;
        }
        if self.fallback_max_waiting > 1000 {
            self.fallback_max_waiting = 1000;
        }
        self.overload_529_cooldown_secs = self.overload_529_cooldown_secs.clamp(1, 86_400);
        for value in [
            &mut self.quota_auto_pause_5h,
            &mut self.quota_auto_pause_7d,
        ] {
            if !value.is_finite() || *value < 0.0 {
                *value = 0.0;
            }
            if *value > 1.0 {
                *value = 1.0;
            }
        }
        for value in [
            &mut self.score_weights.priority,
            &mut self.score_weights.load,
            &mut self.score_weights.queue,
            &mut self.score_weights.error_rate,
            &mut self.score_weights.ttft,
            &mut self.score_weights.reset,
            &mut self.score_weights.quota_headroom,
            &mut self.score_weights.upstream_cost,
            &mut self.score_weights.previous_response,
            &mut self.score_weights.session_sticky,
        ] {
            if !value.is_finite() || *value < 0.0 {
                *value = 0.0;
            }
            if *value > 10.0 {
                *value = 10.0;
            }
        }
        if !self.sticky_escape.ttft_ms.is_finite() || self.sticky_escape.ttft_ms < 0.0 {
            self.sticky_escape.ttft_ms = 15_000.0;
        }
        if !self.sticky_escape.error_rate.is_finite() || self.sticky_escape.error_rate < 0.0 {
            self.sticky_escape.error_rate = 0.5;
        }
        if self.sticky_escape.error_rate > 1.0 {
            self.sticky_escape.error_rate = 1.0;
        }
    }
}

#[derive(Debug, Clone)]
pub struct CandidateAccount {
    pub credential_id: String,
    pub weight: i64,
    pub priority: i64,
    pub enabled: bool,
    pub healthy: bool,
    pub schedule_state: ScheduleState,
    pub cooldown_until: Option<i64>,
    pub active_leases: i64,
    pub concurrency_limit: i64,
    pub error_rate_ewma: f64,
    pub ttft_ewma_ms: f64,
    /// Remaining fraction 0..1 for nearest session window, if known.
    pub quota_remaining: Option<f64>,
    /// Unix seconds when the nearest session window resets, if known.
    pub session_reset_at: Option<i64>,
    /// When the quota snapshot was fetched (unix); used for staleness.
    pub quota_fetched_at: Option<i64>,
    /// Relative upstream cost multiplier (1.0 neutral). Lower is preferred when weight > 0.
    pub upstream_cost_rate: f64,
    /// Last successful use unix (for fallback last_used selection).
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SelectRequest {
    pub routing: RoutingMode,
    pub fixed_credential_id: Option<String>,
    pub previous_response_id: Option<String>,
    pub session_key: Option<String>,
    pub exclude_credential_ids: Vec<String>,
    /// Unix seconds "now" for deterministic tests.
    pub now_unix: i64,
    /// Sticky bindings already resolved: (kind, key_hash) is handled by storage;
    /// here we pass optional bound credential ids for each layer.
    pub previous_response_binding: Option<String>,
    pub session_binding: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOutcome {
    pub credential_id: String,
    pub layer: SelectionLayer,
    pub rebind_previous_response: bool,
    pub rebind_session: bool,
    /// True when a sticky binding existed but was skipped (escape / ineligible).
    pub sticky_escaped: bool,
}
