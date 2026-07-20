use super::scoring::{lottery_weights, score_among_peers};
use super::types::{
    CandidateAccount, PoolSchedulerConfig, RoutingMode, ScheduleState, SelectOutcome,
    SelectRequest, SelectionLayer,
};

/// Whether a sticky-bound account may still be used.
pub fn sticky_eligible(
    candidate: &CandidateAccount,
    config: &PoolSchedulerConfig,
    now_unix: i64,
) -> bool {
    if !is_base_eligible(candidate, now_unix) {
        return false;
    }
    if !config.sticky_escape.enabled {
        return true;
    }
    if candidate.ttft_ewma_ms > config.sticky_escape.ttft_ms {
        return false;
    }
    if candidate.error_rate_ewma > config.sticky_escape.error_rate {
        return false;
    }
    true
}

fn is_base_eligible(candidate: &CandidateAccount, now_unix: i64) -> bool {
    if !candidate.enabled || !candidate.healthy {
        return false;
    }
    if !candidate.schedule_state.is_schedulable() {
        return false;
    }
    if matches!(
        candidate.schedule_state,
        ScheduleState::AuthInvalid | ScheduleState::Entitlement
    ) {
        return false;
    }
    if let Some(until) = candidate.cooldown_until {
        if until > now_unix {
            return false;
        }
    }
    if candidate.active_leases >= candidate.concurrency_limit.max(1) {
        return false;
    }
    true
}

fn excluded(id: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|item| item == id)
}

fn find_candidate<'a>(
    candidates: &'a [CandidateAccount],
    credential_id: &str,
) -> Option<&'a CandidateAccount> {
    candidates
        .iter()
        .find(|item| item.credential_id == credential_id)
}

/// Pure selection. Top-K lottery uses score-shifted weights and a deterministic seed.
pub fn select_account(
    candidates: &[CandidateAccount],
    config: &PoolSchedulerConfig,
    request: &SelectRequest,
    selection_seed: u64,
) -> Option<SelectOutcome> {
    let mut sticky_escaped = false;

    match request.routing {
        RoutingMode::Fixed => {
            let id = request.fixed_credential_id.as_deref()?;
            if excluded(id, &request.exclude_credential_ids) {
                return None;
            }
            let candidate = find_candidate(candidates, id)?;
            if !is_base_eligible(candidate, request.now_unix) {
                return None;
            }
            return Some(SelectOutcome {
                credential_id: id.to_string(),
                layer: SelectionLayer::Fixed,
                rebind_previous_response: false,
                rebind_session: false,
                sticky_escaped: false,
            });
        }
        RoutingMode::Pool => {}
    }

    // 1) previous_response_id sticky
    if request.previous_response_id.is_some() {
        if let Some(bound_id) = request.previous_response_binding.as_deref() {
            if !excluded(bound_id, &request.exclude_credential_ids) {
                if let Some(candidate) = find_candidate(candidates, bound_id) {
                    if sticky_eligible(candidate, config, request.now_unix) {
                        return Some(SelectOutcome {
                            credential_id: bound_id.to_string(),
                            layer: SelectionLayer::PreviousResponse,
                            rebind_previous_response: false,
                            rebind_session: false,
                            sticky_escaped: false,
                        });
                    }
                    sticky_escaped = true;
                } else {
                    sticky_escaped = true;
                }
            }
        }
    }

    // 2) session-hash sticky
    if request.session_key.is_some() {
        if let Some(bound_id) = request.session_binding.as_deref() {
            if !excluded(bound_id, &request.exclude_credential_ids) {
                if let Some(candidate) = find_candidate(candidates, bound_id) {
                    if sticky_eligible(candidate, config, request.now_unix) {
                        return Some(SelectOutcome {
                            credential_id: bound_id.to_string(),
                            layer: SelectionLayer::Session,
                            rebind_previous_response: request.previous_response_id.is_some(),
                            rebind_session: false,
                            sticky_escaped,
                        });
                    }
                    sticky_escaped = true;
                } else {
                    sticky_escaped = true;
                }
            }
        }
    }

    // 3) load-aware Top-K weighted selection
    let eligible: Vec<&CandidateAccount> = candidates
        .iter()
        .filter(|item| {
            !excluded(&item.credential_id, &request.exclude_credential_ids)
                && is_base_eligible(item, request.now_unix)
        })
        .collect();
    if eligible.is_empty() {
        return None;
    }

    let peer_owned: Vec<CandidateAccount> = eligible.iter().map(|c| (*c).clone()).collect();
    let mut scored: Vec<(&CandidateAccount, f64)> = eligible
        .iter()
        .map(|c| {
            (
                *c,
                score_among_peers(c, &peer_owned, config, request.now_unix),
            )
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.0.priority.cmp(&a.0.priority))
            .then_with(|| a.0.credential_id.cmp(&b.0.credential_id))
    });

    let top_k = config.lb_top_k.max(1) as usize;
    let top: Vec<(&CandidateAccount, f64)> = scored.into_iter().take(top_k).collect();
    let scores: Vec<f64> = top.iter().map(|(_, s)| *s).collect();
    let member_weights: Vec<i64> = top.iter().map(|(c, _)| c.weight).collect();
    let lottery = lottery_weights(&scores, &member_weights);
    let selected = weighted_pick_f64(&top, &lottery, selection_seed)?;

    Some(SelectOutcome {
        credential_id: selected.credential_id.clone(),
        layer: SelectionLayer::LoadBalance,
        rebind_previous_response: request.previous_response_id.is_some(),
        rebind_session: request.session_key.is_some(),
        sticky_escaped,
    })
}

fn weighted_pick_f64<'a>(
    candidates: &[(&'a CandidateAccount, f64)],
    weights: &[f64],
    seed: u64,
) -> Option<&'a CandidateAccount> {
    if candidates.is_empty() || weights.is_empty() {
        return None;
    }
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return Some(candidates[0].0);
    }
    // Map seed into [0, total)
    let ticket = (seed as f64 / u64::MAX as f64) * total;
    let mut acc = 0.0;
    for (idx, weight) in weights.iter().enumerate() {
        acc += *weight;
        if ticket < acc {
            return Some(candidates[idx].0);
        }
    }
    candidates.last().map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::types::{
        CandidateAccount, PoolSchedulerConfig, RoutingMode, ScheduleState, SelectRequest,
        SelectionLayer,
    };

    fn candidate(id: &str, weight: i64, priority: i64) -> CandidateAccount {
        CandidateAccount {
            credential_id: id.into(),
            weight,
            priority,
            enabled: true,
            healthy: true,
            schedule_state: ScheduleState::Ready,
            cooldown_until: None,
            active_leases: 0,
            concurrency_limit: 2,
            error_rate_ewma: 0.0,
            ttft_ewma_ms: 0.0,
            quota_remaining: Some(1.0),
            session_reset_at: None,
            quota_fetched_at: Some(0),
        }
    }

    fn req() -> SelectRequest {
        SelectRequest {
            routing: RoutingMode::Pool,
            fixed_credential_id: None,
            previous_response_id: None,
            session_key: None,
            exclude_credential_ids: vec![],
            now_unix: 1_700_000_000,
            previous_response_binding: None,
            session_binding: None,
        }
    }

    #[test]
    fn previous_response_beats_session_and_load_balance() {
        let candidates = vec![candidate("a", 1, 0), candidate("b", 1, 0), candidate("c", 1, 0)];
        let config = PoolSchedulerConfig::default();
        let mut request = req();
        request.previous_response_id = Some("resp_1".into());
        request.session_key = Some("sess_1".into());
        request.previous_response_binding = Some("b".into());
        request.session_binding = Some("c".into());
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.credential_id, "b");
        assert_eq!(outcome.layer, SelectionLayer::PreviousResponse);
        assert!(!outcome.sticky_escaped);
    }

    #[test]
    fn session_beats_load_balance() {
        let candidates = vec![candidate("a", 1, 0), candidate("b", 1, 0)];
        let config = PoolSchedulerConfig::default();
        let mut request = req();
        request.session_key = Some("sess_1".into());
        request.session_binding = Some("b".into());
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.credential_id, "b");
        assert_eq!(outcome.layer, SelectionLayer::Session);
    }

    #[test]
    fn sticky_escape_on_cooldown() {
        let mut bound = candidate("sticky", 1, 100);
        bound.cooldown_until = Some(1_700_000_100);
        let other = candidate("other", 1, 0);
        let candidates = vec![bound, other];
        let config = PoolSchedulerConfig::default();
        let mut request = req();
        request.session_key = Some("sess".into());
        request.session_binding = Some("sticky".into());
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.credential_id, "other");
        assert_eq!(outcome.layer, SelectionLayer::LoadBalance);
        assert!(outcome.rebind_session);
        assert!(outcome.sticky_escaped);
    }

    #[test]
    fn sticky_escape_on_high_error_rate() {
        let mut bound = candidate("sticky", 1, 0);
        bound.error_rate_ewma = 0.9;
        let mut other = candidate("other", 1, 50);
        other.error_rate_ewma = 0.0;
        let candidates = vec![bound, other];
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.error_rate = 2.0;
        let mut request = req();
        request.previous_response_id = Some("resp".into());
        request.previous_response_binding = Some("sticky".into());
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.layer, SelectionLayer::LoadBalance);
        assert_eq!(outcome.credential_id, "other");
        assert!(outcome.rebind_previous_response);
        assert!(outcome.sticky_escaped);
    }

    #[test]
    fn fixed_mode_ignores_pool() {
        let candidates = vec![candidate("a", 10, 100), candidate("b", 1, 0)];
        let config = PoolSchedulerConfig::default();
        let mut request = req();
        request.routing = RoutingMode::Fixed;
        request.fixed_credential_id = Some("b".into());
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.credential_id, "b");
        assert_eq!(outcome.layer, SelectionLayer::Fixed);
    }

    #[test]
    fn cooldown_filters_candidates() {
        let mut a = candidate("a", 1, 0);
        a.cooldown_until = Some(1_700_000_500);
        let b = candidate("b", 1, 0);
        let candidates = vec![a, b];
        let config = PoolSchedulerConfig::default();
        let outcome = select_account(&candidates, &config, &req(), 0).unwrap();
        assert_eq!(outcome.credential_id, "b");
    }

    #[test]
    fn exclude_prevents_reselect() {
        let candidates = vec![candidate("a", 1, 0), candidate("b", 1, 0)];
        let config = PoolSchedulerConfig::default();
        let mut request = req();
        request.exclude_credential_ids = vec!["a".into()];
        let outcome = select_account(&candidates, &config, &request, 0).unwrap();
        assert_eq!(outcome.credential_id, "b");
    }

    #[test]
    fn higher_score_dominates_lottery_most_seeds() {
        let mut a = candidate("a", 1, 100);
        a.error_rate_ewma = 0.0;
        let mut b = candidate("b", 1, 0);
        b.error_rate_ewma = 0.0;
        let candidates = vec![a, b];
        let config = PoolSchedulerConfig {
            lb_top_k: 2,
            ..PoolSchedulerConfig::default()
        };
        // priority weight high → a almost always wins; sample several seeds
        let mut a_wins = 0;
        for seed in [0u64, 1, 2, 3, 4, 5, 10, 100, 1000, 99999] {
            let outcome = select_account(&candidates, &config, &req(), seed).unwrap();
            if outcome.credential_id == "a" {
                a_wins += 1;
            }
        }
        assert!(a_wins >= 7, "expected high-score account to win most draws, got {a_wins}");
    }

    #[test]
    fn auth_invalid_not_selected() {
        let mut bad = candidate("bad", 100, 100);
        bad.schedule_state = ScheduleState::AuthInvalid;
        bad.healthy = false;
        let good = candidate("good", 1, 0);
        let candidates = vec![bad, good];
        let outcome = select_account(&candidates, &PoolSchedulerConfig::default(), &req(), 0).unwrap();
        assert_eq!(outcome.credential_id, "good");
    }

    #[test]
    fn entitlement_distinct_from_ready() {
        let mut blocked = candidate("blocked", 100, 100);
        blocked.schedule_state = ScheduleState::Entitlement;
        blocked.healthy = false;
        let good = candidate("good", 1, 0);
        let candidates = vec![blocked, good];
        let outcome = select_account(&candidates, &PoolSchedulerConfig::default(), &req(), 0).unwrap();
        assert_eq!(outcome.credential_id, "good");
    }

    #[test]
    fn concurrency_full_not_eligible() {
        let mut full = candidate("full", 1, 100);
        full.active_leases = 1;
        full.concurrency_limit = 1;
        let free = candidate("free", 1, 0);
        let candidates = vec![full, free];
        let outcome = select_account(&candidates, &PoolSchedulerConfig::default(), &req(), 0).unwrap();
        assert_eq!(outcome.credential_id, "free");
    }

    #[test]
    fn quota_headroom_influences_selection() {
        let mut rich = candidate("rich", 1, 0);
        rich.quota_remaining = Some(0.95);
        rich.quota_fetched_at = Some(1_700_000_000);
        let mut poor = candidate("poor", 1, 0);
        poor.quota_remaining = Some(0.05);
        poor.quota_fetched_at = Some(1_700_000_000);
        let candidates = vec![rich, poor];
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.quota_headroom = 2.0;
        config.lb_top_k = 1; // force highest score only
        let outcome = select_account(&candidates, &config, &req(), 0).unwrap();
        assert_eq!(outcome.credential_id, "rich");
    }
}
