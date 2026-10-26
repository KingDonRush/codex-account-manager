use std::cmp::Ordering;

use chrono::{DateTime, Utc};

use crate::domain::{
    AccountCard, AccountHealth, CredentialState, DashboardCounts, DashboardSnapshot,
    ManagedProfile, ProfileStatus, QuotaPresentation, Recommendation, RotationStrategy,
    UsageWindow,
};

#[cfg(test)]
pub fn build_snapshot(profiles: Vec<ManagedProfile>, now: DateTime<Utc>) -> DashboardSnapshot {
    build_snapshot_with_strategy(profiles, now, RotationStrategy::BeforeReset)
}

/// Projects local facts without side effects; recommendation never orders navigation.
pub fn build_snapshot_with_strategy(
    profiles: Vec<ManagedProfile>,
    now: DateTime<Utc>,
    strategy: RotationStrategy,
) -> DashboardSnapshot {
    let mut accounts = profiles
        .into_iter()
        .map(|profile| account_card(profile, now))
        .collect::<Vec<_>>();
    // Navigation must not move a click target when polling changes quota or health.
    accounts.sort_by(|left, right| {
        left.profile
            .created_at
            .cmp(&right.profile.created_at)
            .then_with(|| left.profile.id.cmp(&right.profile.id))
    });
    if let Some(index) = recommended_index(&accounts, strategy, now) {
        annotate_recommendation(&mut accounts[index], strategy, now);
    }
    let counts = dashboard_counts(&accounts);
    DashboardSnapshot {
        revision: 0,
        accounts,
        counts,
        generated_at: now,
    }
}

fn dashboard_counts(accounts: &[AccountCard]) -> DashboardCounts {
    DashboardCounts {
        connected: accounts
            .iter()
            .filter(|card| card.health == AccountHealth::Connected)
            .count(),
        attention: accounts
            .iter()
            .filter(|card| card.health == AccountHealth::Attention)
            .count(),
        archived: accounts
            .iter()
            .filter(|card| card.health == AccountHealth::Archived)
            .count(),
    }
}

fn recommended_index(
    accounts: &[AccountCard],
    strategy: RotationStrategy,
    now: DateTime<Utc>,
) -> Option<usize> {
    accounts
        .iter()
        .enumerate()
        .filter(|(_, card)| recommendation_eligible(card))
        .min_by(|(_, left), (_, right)| compare_cards(left, right, strategy, now))
        .map(|(index, _)| index)
        .or_else(|| {
            accounts
                .iter()
                .enumerate()
                .filter(|(_, card)| {
                    card.health == AccountHealth::Connected
                        && !card.profile.is_active_account
                        && card.profile.rotation_enabled
                })
                .filter_map(|(index, card)| card.available_at.map(|at| (index, at)))
                .min_by_key(|(_, at)| *at)
                .map(|(index, _)| index)
        })
}

fn annotate_recommendation(card: &mut AccountCard, strategy: RotationStrategy, now: DateTime<Utc>) {
    let waiting = card.effective_remaining == Some(0.0);
    let renewal = effective_reset_at(card, now);
    card.recommendation = Some(Recommendation {
        label: if waiting {
            "Próxima a liberar saldo"
        } else {
            "Melhor próxima conta"
        }
        .to_owned(),
        effective_remaining: card.effective_remaining.unwrap_or_default(),
        projected_reset: card.projected_reset,
        reason: recommendation_reason(card, strategy, renewal),
        next_reset_at: if waiting { card.available_at } else { renewal },
    });
}

fn account_card(profile: ManagedProfile, now: DateTime<Utc>) -> AccountCard {
    let (health, attention_reason) = health(&profile);
    let (effective_remaining, projected_reset) = effective_remaining(&profile, now);
    let primary_quota = profile
        .rate_limits
        .as_ref()
        .and_then(|limits| limits.primary.as_ref())
        .map(|window| quota_presentation(window, profile.usage_updated_at, now));
    let secondary_quota = profile
        .rate_limits
        .as_ref()
        .and_then(|limits| limits.secondary.as_ref())
        .map(|window| quota_presentation(window, profile.usage_updated_at, now));
    let windows = [primary_quota.as_ref(), secondary_quota.as_ref()];
    let next_reset_at = windows
        .iter()
        .flatten()
        .filter_map(|quota| quota.resets_at)
        .filter(|timestamp| *timestamp > now.timestamp())
        .min()
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0));
    let available_at = available_after(&windows, now);
    AccountCard {
        profile,
        health,
        attention_reason,
        recommendation: None,
        effective_remaining,
        projected_reset,
        primary_quota,
        secondary_quota,
        next_reset_at,
        available_at,
    }
}

fn health(profile: &ManagedProfile) -> (AccountHealth, Option<String>) {
    if profile.status == ProfileStatus::Archived {
        return (AccountHealth::Archived, None);
    }
    if profile.account_id.is_none() {
        return (
            AccountHealth::Attention,
            Some("Conta ainda não conectada".to_owned()),
        );
    }
    match profile.credential_state {
        CredentialState::Ready => (AccountHealth::Connected, None),
        CredentialState::Quarantined => (
            AccountHealth::Attention,
            Some("Todas as sessões salvas estão em quarentena".to_owned()),
        ),
        CredentialState::Missing => (
            AccountHealth::Attention,
            Some("Sessão salva não encontrada".to_owned()),
        ),
        CredentialState::Unavailable => (
            AccountHealth::Attention,
            Some("Não foi possível consultar o cofre de sessões".to_owned()),
        ),
        CredentialState::NotConnected => (
            AccountHealth::Attention,
            Some("Conta ainda não conectada".to_owned()),
        ),
    }
}

fn effective_remaining(profile: &ManagedProfile, now: DateTime<Utc>) -> (Option<f64>, bool) {
    let Some(limits) = profile.rate_limits.as_ref() else {
        return (None, false);
    };
    let windows = [limits.primary.as_ref(), limits.secondary.as_ref()]
        .into_iter()
        .flatten()
        .map(|window| {
            let quota = quota_presentation(window, profile.usage_updated_at, now);
            (quota.remaining, quota.projected)
        })
        .collect::<Vec<_>>();
    let effective = windows
        .iter()
        .filter_map(|(remaining, _)| *remaining)
        .min_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let projected = windows.iter().any(|(_, projected)| *projected);
    (effective, projected)
}

fn quota_presentation(
    window: &UsageWindow,
    observed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> QuotaPresentation {
    let projected = window.resets_at.is_some_and(|timestamp| {
        now.timestamp() >= timestamp
            && observed_at.is_none_or(|observed| observed.timestamp() < timestamp)
    });
    let observed_remaining = window
        .used_percent
        .filter(|value| value.is_finite())
        .map(|used| (100.0 - used).clamp(0.0, 100.0));
    QuotaPresentation {
        remaining: if projected {
            Some(100.0)
        } else {
            observed_remaining
        },
        projected,
        window_duration_mins: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn available_after(
    windows: &[Option<&QuotaPresentation>; 2],
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let exhausted = windows
        .iter()
        .flatten()
        .filter(|window| window.remaining == Some(0.0));
    let resets = exhausted
        .map(|window| window.resets_at.filter(|at| *at > now.timestamp()))
        .collect::<Option<Vec<_>>>()?;
    resets
        .into_iter()
        .max()
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
}

fn recommendation_reason(
    card: &AccountCard,
    strategy: RotationStrategy,
    renewal: Option<DateTime<Utc>>,
) -> String {
    if card.effective_remaining == Some(0.0) {
        return "É a primeira conta cujo saldo deve voltar a ficar disponível.".to_owned();
    }
    let remaining = card.effective_remaining.unwrap_or_default();
    if card.profile.preferred {
        return format!(
            "Conta preferida com {remaining:.0}% utilizável{}.",
            if card.projected_reset {
                " estimado após reset"
            } else {
                " confirmado"
            }
        );
    }
    if card.projected_reset {
        return format!(
            "{remaining:.0}% utilizável estimado após o reset; confirmação em segundo plano."
        );
    }
    if strategy == RotationStrategy::BeforeReset && renewal.is_some() {
        return format!(
            "{remaining:.0}% utilizável; prioriza saldo suficiente com renovação útil mais próxima."
        );
    }
    format!("{remaining:.0}% utilizável na janela mais restrita.")
}

fn compare_cards(
    left: &AccountCard,
    right: &AccountCard,
    strategy: RotationStrategy,
    now: DateTime<Utc>,
) -> Ordering {
    health_rank(&left.health)
        .cmp(&health_rank(&right.health))
        .then_with(|| {
            left.profile
                .is_active_account
                .cmp(&right.profile.is_active_account)
        })
        .then_with(|| {
            right
                .profile
                .rotation_enabled
                .cmp(&left.profile.rotation_enabled)
        })
        .then_with(|| preferred_rank(left).cmp(&preferred_rank(right)))
        .then_with(|| compare_balance(left, right, strategy, now))
        .then_with(|| {
            left.profile
                .last_disconnected_at
                .cmp(&right.profile.last_disconnected_at)
        })
        .then_with(|| left.profile.name.cmp(&right.profile.name))
        .then_with(|| left.profile.id.cmp(&right.profile.id))
}

fn preferred_rank(card: &AccountCard) -> u8 {
    u8::from(!(card.profile.preferred && recommendation_eligible(card)))
}

fn compare_balance(
    left: &AccountCard,
    right: &AccountCard,
    strategy: RotationStrategy,
    now: DateTime<Utc>,
) -> Ordering {
    let remaining = || compare_remaining(right.effective_remaining, left.effective_remaining);
    match strategy {
        RotationStrategy::BeforeReset => quota_band(left)
            .cmp(&quota_band(right))
            .then_with(|| {
                if left.effective_remaining == Some(100.0)
                    || right.effective_remaining == Some(100.0)
                {
                    return Ordering::Equal;
                }
                compare_reset(
                    effective_reset_at(left, now),
                    effective_reset_at(right, now),
                )
            })
            .then_with(remaining),
        RotationStrategy::LargestBalance => remaining().then_with(|| {
            compare_reset(
                effective_reset_at(left, now),
                effective_reset_at(right, now),
            )
        }),
    }
}

// A non-limiting reset does not increase usable balance. Tied bottlenecks must
// both renew; an unknown reset cannot be filled in with a guessed deadline.
fn effective_reset_at(card: &AccountCard, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let remaining = card.effective_remaining?;
    // Unused full windows can report now + duration on every poll. They have
    // nothing to replenish, so these sliding timestamps must not pick a winner.
    if remaining == 100.0 {
        return None;
    }
    [card.primary_quota.as_ref(), card.secondary_quota.as_ref()]
        .into_iter()
        .flatten()
        .filter(|quota| quota.remaining == Some(remaining))
        .try_fold(None, |latest: Option<DateTime<Utc>>, quota| {
            let at = quota
                .resets_at
                .filter(|at| *at > now.timestamp())
                .and_then(|at| DateTime::from_timestamp(at, 0))?;
            Some(Some(latest.map_or(at, |previous| previous.max(at))))
        })
        .flatten()
}

fn compare_reset(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn health_rank(health: &AccountHealth) -> u8 {
    match health {
        AccountHealth::Connected => 0,
        AccountHealth::Attention => 1,
        AccountHealth::Archived => 2,
    }
}

fn quota_band(card: &AccountCard) -> u8 {
    match card.effective_remaining {
        Some(value) if value.round() >= 50.0 => 0,
        Some(value) if value.round() >= 20.0 => 1,
        Some(value) if value > 0.0 => 2,
        Some(_) => 3,
        None => 4,
    }
}

fn compare_remaining(left: Option<f64>, right: Option<f64>) -> Ordering {
    // Match the displayed precision without modifying observed quota or eligibility.
    // Otherwise sub-percent noise changes the suggestion while both cards show 90%.
    match (left.map(f64::round), right.map(f64::round)) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn recommendation_eligible(card: &AccountCard) -> bool {
    card.health == AccountHealth::Connected
        && card.profile.rotation_enabled
        && !card.profile.is_active_account
        && card.effective_remaining.is_some()
        && quota_band(card) <= 2
}

#[cfg(test)]
mod tests;
