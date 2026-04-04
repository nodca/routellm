use rand::{Rng, distributions::WeightedIndex, prelude::Distribution};

use crate::{
    domain::{CandidateView, ChannelRow, ModelRouteRow, RouteDecisionView},
    error::AppError,
};

#[derive(Debug, Clone)]
pub struct CandidateEvaluation {
    pub channel: ChannelRow,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub selected: ChannelRow,
    pub candidates: Vec<CandidateEvaluation>,
}

pub fn decide_route(
    requested_model: &str,
    _route: &ModelRouteRow,
    channels: Vec<ChannelRow>,
    now_ts: i64,
) -> Result<RouteDecision, AppError> {
    let candidates = inspect_candidates(channels, now_ts);
    let mut rng = rand::thread_rng();
    let selected = choose_candidate_with_rng(&candidates, &mut rng)
        .cloned()
        .ok_or_else(|| {
            AppError::NoRoute(format!("no eligible channel for model: {requested_model}"))
        })?;

    Ok(RouteDecision {
        selected,
        candidates,
    })
}

pub fn inspect_candidates(channels: Vec<ChannelRow>, now_ts: i64) -> Vec<CandidateEvaluation> {
    channels
        .into_iter()
        .map(|channel| {
            let (eligible, reason) = if channel.enabled == 0 {
                (false, "channel disabled".to_string())
            } else if channel.account_status != "active" {
                (false, format!("account status={}", channel.account_status))
            } else if channel.site_status != "active" {
                (false, format!("site status={}", channel.site_status))
            } else if channel.supports_responses == 0 {
                (false, "responses not supported".to_string())
            } else if channel.manual_blocked != 0 {
                (false, "manual intervention required".to_string())
            } else if channel.cooldown_until.is_some_and(|until| until > now_ts) {
                (
                    false,
                    format!(
                        "cooling down until {}",
                        channel.cooldown_until.unwrap_or_default()
                    ),
                )
            } else {
                (true, "eligible".to_string())
            };

            CandidateEvaluation {
                channel,
                eligible,
                reason,
            }
        })
        .collect()
}

fn choose_candidate_with_rng<'a, R: Rng + ?Sized>(
    candidates: &'a [CandidateEvaluation],
    rng: &mut R,
) -> Option<&'a ChannelRow> {
    let best_priority = candidates
        .iter()
        .filter(|candidate| candidate.eligible)
        .map(|candidate| candidate.channel.priority)
        .min()?;

    let pool: Vec<&CandidateEvaluation> = candidates
        .iter()
        .filter(|candidate| candidate.eligible && candidate.channel.priority == best_priority)
        .collect();

    let weights: Vec<u64> = pool
        .iter()
        .map(|candidate| candidate.channel.weight.max(1) as u64)
        .collect();

    let distribution = WeightedIndex::new(&weights).ok()?;
    let selected_index = distribution.sample(rng);
    Some(&pool[selected_index].channel)
}

pub fn to_decision_view(
    requested_model: &str,
    route: &ModelRouteRow,
    decision: &RouteDecision,
) -> RouteDecisionView {
    RouteDecisionView {
        requested_model: requested_model.to_string(),
        route_id: route.id,
        routing_strategy: route.routing_strategy.clone(),
        selected_channel_id: decision.selected.channel_id,
        selected_account_id: decision.selected.account_id,
        selected_label: format!(
            "{} @ {} / {}",
            decision.selected.account_label,
            decision.selected.site_name,
            decision.selected.channel_label
        ),
        candidates: decision
            .candidates
            .iter()
            .map(|candidate| CandidateView {
                channel_id: candidate.channel.channel_id,
                account_id: candidate.channel.account_id,
                site_name: candidate.channel.site_name.clone(),
                label: candidate.channel.channel_label.clone(),
                priority: candidate.channel.priority,
                weight: candidate.channel.weight,
                eligible: candidate.eligible,
                reason: candidate.reason.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};

    use super::{choose_candidate_with_rng, inspect_candidates};
    use crate::domain::ChannelRow;

    fn channel(id: i64, priority: i64, weight: i64, cooldown_until: Option<i64>) -> ChannelRow {
        ChannelRow {
            channel_id: id,
            route_id: 1,
            account_id: id,
            account_label: format!("acc-{id}"),
            account_api_key: "token".to_string(),
            account_status: "active".to_string(),
            site_name: format!("site-{id}"),
            site_base_url: "https://example.com".to_string(),
            site_status: "active".to_string(),
            channel_label: "default".to_string(),
            upstream_model: "gpt-5.4".to_string(),
            supports_responses: 1,
            enabled: 1,
            priority,
            weight,
            cooldown_until,
            manual_blocked: 0,
            consecutive_fail_count: 0,
            last_status: None,
            last_error: None,
        }
    }

    #[test]
    fn cooled_down_channels_are_filtered_out() {
        let candidates = inspect_candidates(
            vec![channel(1, 0, 10, Some(200)), channel(2, 0, 10, None)],
            100,
        );
        let eligible: Vec<i64> = candidates
            .iter()
            .filter(|candidate| candidate.eligible)
            .map(|candidate| candidate.channel.channel_id)
            .collect();
        assert_eq!(eligible, vec![2]);
    }

    #[test]
    fn selector_prefers_lower_priority_group() {
        let candidates =
            inspect_candidates(vec![channel(1, 0, 10, None), channel(2, 1, 999, None)], 100);
        let mut rng = StdRng::seed_from_u64(7);
        let selected = choose_candidate_with_rng(&candidates, &mut rng).unwrap();
        assert_eq!(selected.channel_id, 1);
    }
}
