use chrono::{DateTime, Utc};
use rusqlite::Row;

use crate::domain::{
    CredentialGeneration, CredentialGenerationState, CredentialState, LoginOperation, MailProvider,
    ManagedProfile, OperationKind, OperationState, ProfileStatus, RateLimits,
};

pub(super) fn profile_from_row(row: &Row<'_>) -> rusqlite::Result<ManagedProfile> {
    let status: String = row.get(7)?;
    let rate_limits_json: Option<String> = row.get(13)?;
    Ok(ManagedProfile {
        id: row.get(0)?,
        name: row.get(1)?,
        account_id: row.get(2)?,
        email: row.get(3)?,
        mail_provider_id: row.get(4)?,
        mail_url: row.get(5)?,
        open_mail_with_login: false,
        data_dir: row.get(6)?,
        status: ProfileStatus::try_from(status.as_str()).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, error.into())
        })?,
        last_used_at: parse_optional_time(row.get(8)?)?,
        last_error: row.get(9)?,
        plan_type: row.get(12)?,
        rate_limits: rate_limits_json
            .map(|value| serde_json::from_str::<RateLimits>(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    13,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        usage_updated_at: parse_optional_time(row.get(14)?)?,
        is_active_account: false,
        needs_activation: false,
        rotation_enabled: true,
        preferred: false,
        credential_state: CredentialState::try_from(row.get::<_, String>(19)?.as_str()).map_err(
            |error| {
                rusqlite::Error::FromSqlConversionFailure(
                    19,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            },
        )?,
        last_activated_at: parse_optional_time(row.get(15)?)?,
        last_disconnected_at: parse_optional_time(row.get(16)?)?,
        last_browser_opened_at: parse_optional_time(row.get(17)?)?,
        last_browser_authenticated_at: parse_optional_time(row.get(18)?)?,
        created_at: parse_time(row.get(10)?)?,
        updated_at: parse_time(row.get(11)?)?,
    })
}

pub(super) fn provider_from_row(row: &Row<'_>) -> rusqlite::Result<MailProvider> {
    let domains: String = row.get(2)?;
    let aliases: String = row.get(4)?;
    Ok(MailProvider {
        id: row.get(0)?,
        display_name: row.get(1)?,
        domains: serde_json::from_str(&domains).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        default_webmail_url: row.get(3)?,
        aliases: serde_json::from_str(&aliases).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        is_custom: row.get::<_, i64>(5)? != 0,
    })
}

pub(super) fn credential_generation_from_row(
    row: &Row<'_>,
) -> rusqlite::Result<CredentialGeneration> {
    let state_value: String = row.get(3)?;
    let state = CredentialGenerationState::try_from(state_value.as_str()).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, error.into())
    })?;
    Ok(CredentialGeneration {
        generation_id: row.get(1)?,
        state,
    })
}

pub(super) fn operation_from_row(row: &Row<'_>) -> rusqlite::Result<LoginOperation> {
    let state_value: String = row.get(3)?;
    let state = OperationState::try_from(state_value.as_str()).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, error.into())
    })?;
    let kind = OperationKind::try_from(row.get::<_, String>(11)?.as_str()).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, error.into())
    })?;
    let started_at = parse_time(row.get(5)?)?;
    let finished_at = parse_optional_time(row.get(6)?)?;
    let browser_pid = row
        .get::<_, Option<i64>>(9)?
        .map(u32::try_from)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?;
    let duration_ms = finished_at.map(|finished| (finished - started_at).num_milliseconds().max(0));
    let stage_started_at = parse_optional_time(row.get(12)?)?.unwrap_or(started_at);
    Ok(LoginOperation {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        kind,
        reconnect: row.get::<_, i64>(2)? != 0,
        state: state.clone(),
        step: row.get(4)?,
        started_at,
        stage_started_at,
        last_stage_duration_ms: row.get(13)?,
        finished_at,
        error_code: row.get(7)?,
        error_message: row.get(8)?,
        browser_pid,
        runtime_kind: row.get(10)?,
        duration_ms,
        cancelable: kind.is_cancelable_in(&state),
    })
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn parse_optional_time(value: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    value.map(parse_time).transpose()
}
