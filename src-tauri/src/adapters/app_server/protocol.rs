use crate::domain::{
    AccountSnapshot, CreditBalance, RateLimitBucket, RateLimits, ResetCredits, UsageWindow,
};
use crate::error::AppError;
use serde_json::Value;

pub(super) fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}

pub(super) fn response_result(message: Value) -> Result<Value, AppError> {
    if let Some(error) = message.get("error") {
        return Err(AppError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-32000),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("resposta de erro sem mensagem")
                .to_owned(),
        });
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

pub(super) fn is_login_completion(message: &Value, login_id: &str) -> bool {
    if message.get("method").and_then(Value::as_str) != Some("account/login/completed") {
        return false;
    }
    let Some(params) = message.get("params").and_then(Value::as_object) else {
        return false;
    };
    match params.get("loginId") {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value == login_id,
        Some(_) => false,
    }
}

pub(super) fn account_update_matches_login(params: &serde_json::Map<String, Value>) -> bool {
    match params.get("authMode") {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value == "chatgpt",
        Some(_) => false,
    }
}

pub(super) fn parse_login_completion(message: &Value) -> Result<(), AppError> {
    let params = message
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::AppServer("notificação de login inválida".to_owned()))?;
    if params.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let message = params
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("login OAuth não concluído");
    if message.to_ascii_lowercase().contains("cancel") {
        return Err(AppError::Cancelled);
    }
    Err(AppError::AppServer(message.to_owned()))
}

pub(super) fn notification_matches(
    message: &Value,
    method: &str,
    predicate: &impl Fn(&serde_json::Map<String, Value>) -> bool,
) -> bool {
    if message.get("method").and_then(Value::as_str) != Some(method) {
        return false;
    }
    message
        .get("params")
        .and_then(Value::as_object)
        .is_some_and(predicate)
}

pub(super) fn session_snapshot(
    account: &Value,
    limits: &Value,
) -> Result<AccountSnapshot, AppError> {
    let account = account
        .get("account")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::SessionRenewalRequired(
            "O Codex não retornou uma identidade nesta consulta. A sessão precisa ser renovada/validada antes de concluir; isso sozinho não prova que seja necessário refazer o login.".to_owned(),
        ))?;
    if account
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "chatgpt")
    {
        return Err(AppError::AuthenticationRequired(
            "Esta sessão não é uma conta ChatGPT.".to_owned(),
        ));
    }
    Ok(AccountSnapshot {
        email: non_empty_string(account.get("email")),
        plan_type: non_empty_string(account.get("planType")),
        rate_limits: parse_rate_limits(limits),
    })
}

pub(super) fn optional_limits(response: Result<Value, AppError>) -> Result<Value, AppError> {
    match response {
        Ok(value) => Ok(value),
        Err(error) if error.is_authentication_failure() => Err(error),
        Err(_) => Ok(Value::Null),
    }
}

pub(super) fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn parse_rate_limits(response: &Value) -> Option<RateLimits> {
    let limits = response
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .and_then(|limits| limits.get("codex"))
        .and_then(Value::as_object)
        .or_else(|| response.get("rateLimits").and_then(Value::as_object));
    let rate_limits = RateLimits {
        primary: limits
            .and_then(|value| value.get("primary"))
            .and_then(parse_usage_window),
        secondary: limits
            .and_then(|value| value.get("secondary"))
            .and_then(parse_usage_window),
        rate_limit_reached_type: limits
            .and_then(|value| value.get("rateLimitReachedType"))
            .and_then(|value| non_empty_string(Some(value))),
        reset_credits: parse_reset_credits(response.get("rateLimitResetCredits")),
        buckets: response
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .map(|buckets| {
                buckets
                    .iter()
                    .filter_map(|(id, value)| {
                        parse_bucket(value).map(|bucket| (id.clone(), bucket))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        credits: limits
            .and_then(|value| value.get("credits"))
            .and_then(parse_credits),
    };
    if rate_limits.primary.is_none()
        && rate_limits.secondary.is_none()
        && rate_limits.rate_limit_reached_type.is_none()
        && rate_limits.reset_credits.is_none()
        && rate_limits.buckets.is_empty()
        && rate_limits.credits.is_none()
    {
        return None;
    }
    Some(rate_limits)
}

fn parse_bucket(value: &Value) -> Option<RateLimitBucket> {
    let object = value.as_object()?;
    Some(RateLimitBucket {
        limit_id: non_empty_string(object.get("limitId")),
        limit_name: non_empty_string(object.get("limitName")),
        plan_type: non_empty_string(object.get("planType")),
        primary: object.get("primary").and_then(parse_usage_window),
        secondary: object.get("secondary").and_then(parse_usage_window),
        credits: object.get("credits").and_then(parse_credits),
        rate_limit_reached_type: non_empty_string(object.get("rateLimitReachedType")),
    })
}

fn parse_credits(value: &Value) -> Option<CreditBalance> {
    let object = value.as_object()?;
    Some(CreditBalance {
        has_credits: object.get("hasCredits").and_then(Value::as_bool),
        unlimited: object.get("unlimited").and_then(Value::as_bool),
        balance: object.get("balance").and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_number().map(ToString::to_string))
        }),
    })
}

pub(super) fn parse_reset_credits(value: Option<&Value>) -> Option<ResetCredits> {
    let object = value?.as_object()?;
    let available_count = number_as_u64(object.get("availableCount"))
        .or_else(|| number_as_u64(object.get("available_count")));
    available_count.map(|available_count| ResetCredits {
        available_count: Some(available_count),
    })
}

pub(super) fn parse_usage_window(value: &Value) -> Option<UsageWindow> {
    let window = value.as_object()?;
    let used_percent = window
        .get("usedPercent")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0));
    let duration = number_as_u64(window.get("windowDurationMins"));
    let resets_at = number_as_i64(window.get("resetsAt"));
    if used_percent.is_none() && duration.is_none() && resets_at.is_none() {
        return None;
    }
    Some(UsageWindow {
        used_percent,
        window_duration_mins: duration,
        resets_at,
    })
}

pub(super) fn number_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map(|number| number as u64)
        })
    })
}

pub(super) fn number_as_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value.as_i64().or_else(|| {
            value
                .as_f64()
                .filter(|number| number.is_finite())
                .map(|number| number as i64)
        })
    })
}
