use chrono::{TimeZone, Utc};

use super::build_snapshot;
use super::build_snapshot_with_strategy;
use crate::domain::{
    CredentialState, ManagedProfile, ProfileStatus, RateLimits, ResetCredits, RotationStrategy,
    UsageWindow,
};

fn recommended(snapshot: &crate::domain::DashboardSnapshot) -> &crate::domain::AccountCard {
    snapshot
        .accounts
        .iter()
        .find(|card| card.recommendation.is_some())
        .expect("recommendation")
}

fn profile(
    name: &str,
    short_remaining: f64,
    weekly_remaining: f64,
    disconnected: i64,
) -> ManagedProfile {
    ManagedProfile {
        id: name.to_owned(),
        name: name.to_owned(),
        account_id: Some(format!("account-{name}")),
        email: Some(format!("{name}@example.com")),
        mail_provider_id: None,
        mail_url: None,
        open_mail_with_login: false,
        data_dir: format!("/profiles/{name}"),
        status: ProfileStatus::Active,
        last_used_at: None,
        last_activated_at: None,
        last_disconnected_at: Utc.timestamp_opt(disconnected, 0).single(),
        last_browser_opened_at: None,
        last_browser_authenticated_at: None,
        last_error: None,
        plan_type: Some("plus".to_owned()),
        rate_limits: Some(RateLimits {
            primary: Some(UsageWindow {
                used_percent: Some(100.0 - short_remaining),
                window_duration_mins: Some(300),
                resets_at: Some(2_000_000_000),
            }),
            secondary: Some(UsageWindow {
                used_percent: Some(100.0 - weekly_remaining),
                window_duration_mins: Some(10_080),
                resets_at: Some(2_000_000_000),
            }),
            rate_limit_reached_type: None,
            reset_credits: Some(ResetCredits {
                available_count: Some(0),
            }),
            ..RateLimits::default()
        }),
        usage_updated_at: None,
        is_active_account: false,
        needs_activation: false,
        rotation_enabled: true,
        preferred: false,
        credential_state: CredentialState::Ready,
        created_at: Utc.timestamp_opt(1, 0).single().expect("created"),
        updated_at: Utc.timestamp_opt(1, 0).single().expect("updated"),
    }
}

#[test]
fn excluded_and_exhausted_preferred_accounts_are_not_recommended() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut excluded = profile("excluded", 100.0, 100.0, 1);
    excluded.rotation_enabled = false;
    excluded.preferred = true;
    let mut empty = profile("empty", 0.0, 90.0, 1);
    empty.preferred = true;
    let snapshot = build_snapshot(vec![excluded, empty, profile("usable", 30.0, 50.0, 1)], now);
    let recommended = snapshot
        .accounts
        .iter()
        .find(|card| card.recommendation.is_some())
        .expect("recommendation");
    assert_eq!(recommended.profile.name, "usable");
}

#[test]
fn selected_strategy_changes_the_recommendation_without_mutating_quota() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut soon = profile("soon", 60.0, 80.0, 1);
    soon.rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .resets_at = Some(now.timestamp() + 60);
    let largest = profile("largest", 90.0, 90.0, 1);
    let reset = build_snapshot_with_strategy(
        vec![soon.clone(), largest.clone()],
        now,
        RotationStrategy::BeforeReset,
    );
    let balance =
        build_snapshot_with_strategy(vec![soon, largest], now, RotationStrategy::LargestBalance);
    assert_eq!(recommended(&reset).profile.name, "soon");
    assert_eq!(recommended(&balance).profile.name, "largest");
    assert_eq!(recommended(&balance).effective_remaining, Some(90.0));
}

#[test]
fn preferred_usable_account_overrides_the_normal_rank() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut preferred = profile("preferred", 10.0, 50.0, 1);
    preferred.preferred = true;
    let snapshot = build_snapshot(vec![profile("normal", 90.0, 90.0, 1), preferred], now);
    assert_eq!(recommended(&snapshot).profile.name, "preferred");
    assert!(recommended(&snapshot)
        .recommendation
        .as_ref()
        .expect("recommendation")
        .reason
        .contains("preferida"));
}

#[test]
fn a_near_non_limiting_reset_does_not_outrank_a_useful_renewal() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut weekly_limited = profile("weekly", 90.0, 60.0, 1);
    let limits = weekly_limited.rate_limits.as_mut().expect("limits");
    limits.primary.as_mut().expect("primary").resets_at = Some(now.timestamp() + 60);
    limits.secondary.as_mut().expect("secondary").resets_at = Some(now.timestamp() + 3600);
    let mut short_limited = profile("short", 60.0, 90.0, 1);
    short_limited
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .resets_at = Some(now.timestamp() + 1200);
    let snapshot = build_snapshot(vec![weekly_limited, short_limited], now);
    assert_eq!(recommended(&snapshot).profile.name, "short");
    assert_eq!(
        recommended(&snapshot)
            .recommendation
            .as_ref()
            .expect("recommendation")
            .next_reset_at
            .map(|at| at.timestamp()),
        Some(now.timestamp() + 1200)
    );
}

#[test]
fn effective_quota_uses_the_weaker_window() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let snapshot = build_snapshot(
        vec![
            profile("balanced", 70.0, 70.0, 20),
            profile("unbalanced", 90.0, 10.0, 10),
        ],
        now,
    );

    assert_eq!(recommended(&snapshot).profile.name, "balanced");
    assert_eq!(
        recommended(&snapshot)
            .recommendation
            .as_ref()
            .map(|item| item.label.as_str()),
        Some("Melhor próxima conta")
    );
}

#[test]
fn remaining_balance_precedes_disconnect_age_when_resets_match() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let snapshot = build_snapshot(
        vec![
            profile("newer", 82.0, 71.0, 200),
            profile("older", 80.0, 70.0, 100),
        ],
        now,
    );

    assert_eq!(recommended(&snapshot).profile.name, "newer");
}

#[test]
fn expired_window_projects_full_capacity() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut reset = profile("reset", 0.0, 0.0, 10);
    let limits = reset.rate_limits.as_mut().expect("limits");
    limits.primary.as_mut().expect("primary").resets_at = Some(now.timestamp() - 1);
    limits.secondary.as_mut().expect("secondary").resets_at = Some(now.timestamp() - 1);

    let snapshot = build_snapshot(vec![reset], now);

    assert_eq!(snapshot.accounts[0].effective_remaining, Some(100.0));
    assert!(snapshot.accounts[0].projected_reset);
}

#[test]
fn navigation_has_a_stable_order_separate_from_rotation_ranking() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut active = profile("active", 99.0, 99.0, 1);
    active.is_active_account = true;
    let mut unknown = profile("unknown", 80.0, 80.0, 50);
    unknown.rate_limits = None;
    let mut attention = profile("attention", 100.0, 100.0, 1);
    attention.credential_state = CredentialState::Missing;
    let mut archived = profile("archived", 100.0, 100.0, 1);
    archived.status = ProfileStatus::Archived;

    let snapshot = build_snapshot(
        vec![
            active,
            profile("high-newer", 95.0, 90.0, 200),
            profile("mid", 45.0, 40.0, 20),
            archived,
            profile("exhausted", 0.0, 0.0, 5),
            profile("high-older", 75.0, 70.0, 100),
            unknown,
            attention,
            profile("low", 19.0, 10.0, 10),
        ],
        now,
    );
    let order = snapshot
        .accounts
        .iter()
        .map(|card| card.profile.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        order,
        vec![
            "active",
            "archived",
            "attention",
            "exhausted",
            "high-newer",
            "high-older",
            "low",
            "mid",
            "unknown",
        ]
    );
    assert_eq!(
        recommended(&snapshot)
            .recommendation
            .as_ref()
            .map(|recommendation| recommendation.label.as_str()),
        Some("Melhor próxima conta")
    );
    assert_eq!(recommended(&snapshot).profile.name, "high-newer");
    assert!(snapshot
        .accounts
        .iter()
        .filter(|card| card.profile.is_active_account
            || card.health != crate::domain::AccountHealth::Connected)
        .all(|card| card.recommendation.is_none()));
}

#[test]
fn week_away_recalculates_both_windows_and_recommends_without_activation() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut rested = profile("rested", 0.0, 84.0, 10);
    rested.usage_updated_at = Some(now - chrono::Duration::days(8));
    let limits = rested.rate_limits.as_mut().expect("limits");
    limits.primary.as_mut().expect("short").resets_at = Some(now.timestamp() - 7 * 86400);
    limits.secondary.as_mut().expect("week").resets_at = Some(now.timestamp() - 1);
    let snapshot = build_snapshot(vec![rested], now);
    let card = &snapshot.accounts[0];
    assert_eq!(card.effective_remaining, Some(100.0));
    assert!(card.projected_reset);
    assert!(card.recommendation.is_some());
    assert!(!card.profile.is_active_account);
    assert_eq!(
        card.profile
            .rate_limits
            .as_ref()
            .expect("raw")
            .primary
            .as_ref()
            .expect("raw short")
            .used_percent,
        Some(100.0)
    );
}

#[test]
fn refreshed_sample_after_reset_is_confirmed_not_projected() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut confirmed = profile("confirmed", 100.0, 100.0, 10);
    confirmed.usage_updated_at = Some(now);
    let limits = confirmed.rate_limits.as_mut().expect("limits");
    limits.primary.as_mut().expect("short").resets_at = Some(now.timestamp() - 1);
    let snapshot = build_snapshot(vec![confirmed], now);
    assert!(
        !snapshot.accounts[0]
            .primary_quota
            .as_ref()
            .expect("quota")
            .projected
    );
}

#[test]
fn next_reset_breaks_ties_inside_a_usable_balance_band() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut soon = profile("soon", 70.0, 80.0, 200);
    soon.rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("short")
        .resets_at = Some(now.timestamp() + 60);
    let snapshot = build_snapshot(vec![profile("later", 95.0, 95.0, 100), soon], now);
    assert_eq!(recommended(&snapshot).profile.name, "soon");
    assert!(recommended(&snapshot)
        .recommendation
        .as_ref()
        .expect("reason")
        .reason
        .contains("renovação"));
}

#[test]
fn next_available_account_waits_for_the_last_exhausted_window() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut exhausted = profile("exhausted", 0.0, 0.0, 10);
    let limits = exhausted.rate_limits.as_mut().expect("limits");
    limits.primary.as_mut().expect("short").resets_at = Some(now.timestamp() + 60);
    limits.secondary.as_mut().expect("week").resets_at = Some(now.timestamp() + 3600);
    let snapshot = build_snapshot(vec![exhausted], now);
    let card = &snapshot.accounts[0];
    assert_eq!(
        card.available_at.map(|at| at.timestamp()),
        Some(now.timestamp() + 3600)
    );
    assert_eq!(
        card.recommendation.as_ref().expect("next").label,
        "Próxima a liberar saldo"
    );
}

#[test]
fn transient_operation_error_does_not_invalidate_a_ready_credential() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut ready = profile("ready", 50.0, 70.0, 10);
    ready.last_error = Some("App Server: timeout de rede".to_owned());
    assert_eq!(build_snapshot(vec![ready], now).counts.connected, 1);
}

#[test]
fn navigation_order_is_independent_of_quota_refresh_and_recommendation() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let first = profile("alpha", 90.0, 90.0, 1);
    let mut second = profile("zeta", 70.0, 70.0, 1);
    let before = build_snapshot(vec![second.clone(), first.clone()], now);
    second
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .used_percent = Some(0.0);
    second
        .rate_limits
        .as_mut()
        .expect("limits")
        .secondary
        .as_mut()
        .expect("secondary")
        .used_percent = Some(0.0);
    second.usage_updated_at = Some(now + chrono::Duration::seconds(10));
    let after = build_snapshot(vec![first, second], now + chrono::Duration::seconds(10));
    let ids = |snapshot: &crate::domain::DashboardSnapshot| {
        snapshot
            .accounts
            .iter()
            .map(|card| card.profile.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&before), vec!["alpha", "zeta"]);
    assert_eq!(ids(&after), ids(&before));
    assert_eq!(
        before
            .accounts
            .iter()
            .find(|card| card.recommendation.is_some())
            .expect("before")
            .profile
            .id,
        "alpha"
    );
    assert_eq!(
        after
            .accounts
            .iter()
            .find(|card| card.recommendation.is_some())
            .expect("after")
            .profile
            .id,
        "zeta"
    );
}

#[test]
fn equal_facts_and_names_have_a_stable_recommendation_across_input_orders() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut first = profile("alpha", 90.0, 90.0, 1);
    let mut second = profile("zeta", 90.0, 90.0, 1);
    first.name = "Nome repetido".to_owned();
    second.name = first.name.clone();
    let before = build_snapshot(vec![second.clone(), first.clone()], now);
    first.usage_updated_at = Some(now + chrono::Duration::seconds(10));
    second.usage_updated_at = first.usage_updated_at;
    let after = build_snapshot(vec![first, second], now + chrono::Duration::seconds(10));
    for snapshot in [before, after] {
        assert_eq!(
            snapshot
                .accounts
                .iter()
                .find(|card| card.recommendation.is_some())
                .expect("recommendation")
                .profile
                .id,
            "alpha"
        );
    }
}

#[test]
fn invisible_fractional_balance_changes_do_not_shuffle_the_recommendation() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let first = profile("alpha", 90.2, 95.0, 1);
    let mut second = profile("zeta", 90.1, 95.0, 1);
    let strategy = RotationStrategy::LargestBalance;
    assert_eq!(
        recommended(&build_snapshot_with_strategy(
            vec![first.clone(), second.clone()],
            now,
            strategy
        ))
        .profile
        .id,
        "alpha"
    );
    second
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .used_percent = Some(9.7);
    assert_eq!(
        recommended(&build_snapshot_with_strategy(
            vec![first.clone(), second.clone()],
            now,
            strategy
        ))
        .profile
        .id,
        "alpha"
    );
    second
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .used_percent = Some(9.0);
    assert_eq!(
        recommended(&build_snapshot_with_strategy(
            vec![first, second],
            now,
            strategy
        ))
        .profile
        .id,
        "zeta"
    );
}

#[test]
fn full_unused_windows_do_not_recommend_the_last_account_polled() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut first = profile("alpha", 100.0, 100.0, 1);
    let mut second = profile("zeta", 100.0, 100.0, 1);
    let reset = |profile: &mut ManagedProfile, shift: i64| {
        let limits = profile.rate_limits.as_mut().expect("limits");
        limits.primary.as_mut().expect("primary").resets_at =
            Some(now.timestamp() + 18_000 + shift);
        limits.secondary.as_mut().expect("secondary").resets_at =
            Some(now.timestamp() + 604_800 + shift);
        profile.usage_updated_at = Some(now + chrono::Duration::seconds(shift));
    };
    for turn in 0..4 {
        reset(&mut first, turn * 20 + 10);
        reset(&mut second, turn * 20);
        assert_eq!(
            recommended(&build_snapshot(vec![second.clone(), first.clone()], now))
                .profile
                .id,
            "alpha"
        );
        reset(&mut second, turn * 20 + 20);
        assert_eq!(
            recommended(&build_snapshot(vec![first.clone(), second.clone()], now))
                .profile
                .id,
            "alpha"
        );
    }
}

#[test]
fn fractional_noise_at_a_displayed_band_boundary_is_not_a_material_priority_change() {
    let now = Utc.timestamp_opt(1_800_000_000, 0).single().expect("now");
    let mut first = profile("alpha", 50.1, 90.0, 1);
    let second = profile("zeta", 50.1, 90.0, 1);
    first
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .resets_at = Some(now.timestamp() + 120);
    let before = build_snapshot(vec![first.clone(), second.clone()], now);
    first
        .rate_limits
        .as_mut()
        .expect("limits")
        .primary
        .as_mut()
        .expect("primary")
        .used_percent = Some(50.1);
    let after = build_snapshot(vec![first, second], now);
    assert_eq!(recommended(&before).profile.id, "alpha");
    assert_eq!(recommended(&after).profile.id, "alpha");
}
