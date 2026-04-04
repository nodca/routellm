use crate::{
    domain::{CandidateView, ChannelRow, ModelRouteRow, RouteDecisionView},
    error::AppError,
    protocol::{Protocol, compatibility_cost},
};

#[derive(Debug, Clone)]
pub struct CandidateEvaluation {
    pub channel: ChannelRow,
    pub eligible: bool,
    pub reason: String,
    pub protocol_cost: Option<u8>,
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
    request_protocol: Protocol,
    now_ts: i64,
) -> Result<RouteDecision, AppError> {
    let candidates = inspect_candidates(channels, Some(request_protocol), now_ts);
    let selected = choose_candidate(&candidates).cloned().ok_or_else(|| {
        AppError::NoRoute(format!("no eligible channel for model: {requested_model}"))
    })?;

    Ok(RouteDecision {
        selected,
        candidates,
    })
}

pub fn ordered_eligible_channels(candidates: &[CandidateEvaluation]) -> Vec<ChannelRow> {
    let mut eligible = candidates
        .iter()
        .filter(|candidate| candidate.eligible)
        .collect::<Vec<_>>();

    eligible.sort_by_key(|candidate| {
        (
            candidate.channel.priority,
            candidate.protocol_cost.unwrap_or(u8::MAX),
            candidate.channel.avg_latency_ms.unwrap_or(i64::MAX),
            candidate.channel.channel_id,
        )
    });

    eligible
        .into_iter()
        .map(|candidate| candidate.channel.clone())
        .collect()
}

pub fn inspect_candidates(
    channels: Vec<ChannelRow>,
    request_protocol: Option<Protocol>,
    now_ts: i64,
) -> Vec<CandidateEvaluation> {
    channels
        .into_iter()
        .map(|channel| {
            let parsed_protocol = Protocol::parse(&channel.protocol);
            let (eligible, reason, protocol_cost) = if channel.enabled == 0 {
                (false, "channel disabled".to_string(), None)
            } else if channel.account_status != "active" {
                (
                    false,
                    format!("account status={}", channel.account_status),
                    None,
                )
            } else if channel.site_status != "active" {
                (false, format!("site status={}", channel.site_status), None)
            } else if channel.manual_blocked != 0 {
                (false, "manual intervention required".to_string(), None)
            } else if channel.cooldown_until.is_some_and(|until| until > now_ts) {
                (
                    false,
                    format!(
                        "cooling down until {}",
                        channel.cooldown_until.unwrap_or_default()
                    ),
                    None,
                )
            } else if let Ok(channel_protocol) = parsed_protocol {
                match request_protocol {
                    Some(request_protocol) => {
                        match compatibility_cost(channel_protocol, request_protocol) {
                            Some(cost) => (
                                true,
                                if cost == 0 {
                                    "eligible".to_string()
                                } else {
                                    "eligible via chat->responses adapter".to_string()
                                },
                                Some(cost),
                            ),
                            None => (
                                false,
                                format!(
                                    "protocol mismatch: request={} channel={}",
                                    request_protocol.as_str(),
                                    channel_protocol.as_str()
                                ),
                                None,
                            ),
                        }
                    }
                    None => (true, "eligible".to_string(), Some(0)),
                }
            } else {
                (
                    false,
                    format!("invalid channel protocol `{}`", channel.protocol),
                    None,
                )
            };

            CandidateEvaluation {
                channel,
                eligible,
                reason,
                protocol_cost,
            }
        })
        .collect()
}

fn choose_candidate(candidates: &[CandidateEvaluation]) -> Option<&ChannelRow> {
    let ordered = ordered_eligible_channels(candidates);
    let selected_id = ordered.first()?.channel_id;
    candidates
        .iter()
        .find(|candidate| candidate.channel.channel_id == selected_id)
        .map(|candidate| &candidate.channel)
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
                protocol: candidate.channel.protocol.clone(),
                priority: candidate.channel.priority,
                eligible: candidate.eligible,
                reason: candidate.reason.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_candidate, inspect_candidates};
    use crate::domain::ChannelRow;
    use crate::protocol::Protocol;

    fn channel(id: i64, priority: i64, protocol: &str, cooldown_until: Option<i64>) -> ChannelRow {
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
            protocol: protocol.to_string(),
            enabled: 1,
            priority,
            avg_latency_ms: None,
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
            vec![
                channel(1, 0, "responses", Some(200)),
                channel(2, 0, "responses", None),
            ],
            Some(Protocol::Responses),
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
    fn selector_prefers_priority_before_protocol_cost() {
        let candidates = inspect_candidates(
            vec![
                channel(1, 0, "responses", None),
                channel(2, 1, "chat_completions", None),
            ],
            Some(Protocol::ChatCompletions),
            100,
        );
        let selected = choose_candidate(&candidates).unwrap();
        assert_eq!(selected.channel_id, 1);
    }

    #[test]
    fn selector_prefers_lower_priority_within_same_protocol_group() {
        let candidates = inspect_candidates(
            vec![
                channel(1, 0, "responses", None),
                channel(2, 1, "responses", None),
            ],
            Some(Protocol::Responses),
            100,
        );
        let selected = choose_candidate(&candidates).unwrap();
        assert_eq!(selected.channel_id, 1);
    }

    #[test]
    fn selector_prefers_lower_latency_within_same_priority_and_protocol_group() {
        let mut fast = channel(1, 0, "responses", None);
        fast.avg_latency_ms = Some(120);
        let mut slow = channel(2, 0, "responses", None);
        slow.avg_latency_ms = Some(420);

        let candidates = inspect_candidates(vec![slow, fast], Some(Protocol::Responses), 100);
        let selected = choose_candidate(&candidates).unwrap();
        assert_eq!(selected.channel_id, 1);
    }

    #[test]
    fn admin_inspection_ignores_protocol_mismatch() {
        let candidates = inspect_candidates(vec![channel(1, 0, "messages", None)], None, 100);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].eligible);
    }
}
