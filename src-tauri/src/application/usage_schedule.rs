use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::domain::{CredentialState, ManagedProfile, ProfileStatus};

const ACTIVE_INTERVAL_SECS: i64 = 10;
const OTHER_INTERVAL_SECS: i64 = 10;

#[derive(Default)]
pub struct UsageSchedule {
    attempts: HashMap<String, Attempt>,
}

struct Attempt {
    next_at: DateTime<Utc>,
    failures: u32,
}

impl UsageSchedule {
    pub fn next<'a>(
        &self,
        profiles: &'a [ManagedProfile],
        now: DateTime<Utc>,
    ) -> Option<&'a ManagedProfile> {
        profiles
            .iter()
            .filter(|profile| self.is_due(profile, now))
            .min_by_key(|profile| (profile.usage_updated_at, !profile.is_active_account))
    }

    pub fn record(&mut self, profile: &ManagedProfile, now: DateTime<Utc>, success: bool) {
        let failures = if success {
            0
        } else {
            self.attempts
                .get(&profile.id)
                .map_or(1, |last| (last.failures + 1).min(5))
        };
        let interval = if failures > 0 {
            (20 * 2_i64.pow(failures - 1)).min(300)
        } else if profile.is_active_account {
            ACTIVE_INTERVAL_SECS
        } else {
            OTHER_INTERVAL_SECS
        };
        self.attempts.insert(
            profile.id.clone(),
            Attempt {
                next_at: now + Duration::seconds(interval),
                failures,
            },
        );
    }

    fn is_due(&self, profile: &ManagedProfile, now: DateTime<Utc>) -> bool {
        if profile.status != ProfileStatus::Active
            || profile.account_id.is_none()
            || profile.credential_state == CredentialState::Missing
        {
            return false;
        }
        if let Some(last) = self.attempts.get(&profile.id) {
            if now < last.next_at {
                return false;
            }
        }
        let interval = if profile.is_active_account {
            ACTIVE_INTERVAL_SECS
        } else {
            OTHER_INTERVAL_SECS
        };
        profile.usage_updated_at.is_none_or(|updated| {
            (now - updated).num_seconds() >= interval || reset_crossed(profile, updated, now)
        })
    }
}

fn reset_crossed(profile: &ManagedProfile, observed: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let Some(limits) = profile.rate_limits.as_ref() else {
        return false;
    };
    [limits.primary.as_ref(), limits.secondary.as_ref()]
        .iter()
        .flatten()
        .filter_map(|window| window.resets_at)
        .any(|timestamp| timestamp > observed.timestamp() && timestamp <= now.timestamp())
}

#[cfg(test)]
mod tests {
    use super::UsageSchedule;
    use crate::application::test_support::profile;
    use chrono::{Duration, Utc};

    #[test]
    fn active_and_inactive_accounts_are_both_sampled_every_ten_seconds() {
        let now = Utc::now();
        let mut active = profile("active", now);
        active.is_active_account = true;
        active.usage_updated_at = Some(now - Duration::minutes(3));
        let mut other = profile("other", now);
        other.usage_updated_at = active.usage_updated_at;
        let accounts = [other, active];
        let schedule = UsageSchedule::default();
        assert_eq!(
            schedule.next(&accounts, now).map(|item| item.name.as_str()),
            Some("active")
        );
        assert!(schedule.next(&accounts[..1], now).is_some());
    }

    #[test]
    fn failed_sampling_backs_off_and_never_spins() {
        let now = Utc::now();
        let accounts = [profile("saved", now)];
        let mut schedule = UsageSchedule::default();
        schedule.record(&accounts[0], now, false);
        assert!(schedule
            .next(&accounts, now + Duration::seconds(19))
            .is_none());
        let retry = now + Duration::seconds(20);
        assert!(schedule.next(&accounts, retry).is_some());
        schedule.record(&accounts[0], retry, false);
        assert!(schedule
            .next(&accounts, retry + Duration::seconds(39))
            .is_none());
        assert!(schedule
            .next(&accounts, retry + Duration::seconds(40))
            .is_some());
    }
}
