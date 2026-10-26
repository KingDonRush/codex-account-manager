use crate::domain::RuntimeThread;
use crate::error::AppError;
use serde_json::Value;

pub(super) fn parse_thread(
    id: &str,
    data: &Value,
    goal: &Value,
    terminals: &Value,
) -> Result<RuntimeThread, AppError> {
    let thread = data
        .get("thread")
        .ok_or_else(|| unknown("Metadados de chat ausentes."))?;
    let state = thread
        .pointer("/status/type")
        .and_then(Value::as_str)
        .ok_or_else(|| unknown("Atividade de chat desconhecida."))?;
    let active_goal = goal_is_active(goal)?;
    let terminal_count = complete_list(terminals)?.len();
    Ok(RuntimeThread {
        id: id.to_owned(),
        name: thread
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        state: state.to_owned(),
        active_goal,
        background_terminals: terminal_count,
    })
}

pub(super) fn complete_list(value: &Value) -> Result<&[Value], AppError> {
    if !value.get("nextCursor").is_some_and(Value::is_null) {
        return Err(unknown(
            "Lista de atividade incompleta; encerramento automático bloqueado.",
        ));
    }
    value
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| unknown("Lista de atividade ausente."))
}

fn goal_is_active(value: &Value) -> Result<bool, AppError> {
    let goal = value
        .get("goal")
        .ok_or_else(|| unknown("Atividade do objetivo desconhecida."))?;
    if goal.is_null() {
        return Ok(false);
    }
    match goal.get("status").and_then(Value::as_str) {
        Some("active") => Ok(true),
        Some("paused" | "blocked" | "complete" | "usageLimited" | "budgetLimited") => Ok(false),
        _ => Err(unknown(
            "Estado do objetivo desconhecido; sessão preservada.",
        )),
    }
}

fn unknown(message: &str) -> AppError {
    AppError::Runtime(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_and_unknown_activity_never_mean_idle() {
        assert!(goal_is_active(&json!({})).is_err());
        assert!(goal_is_active(&json!({"goal": {}})).is_err());
        assert!(goal_is_active(&json!({"goal": {"status": "newState"}})).is_err());
        assert!(!goal_is_active(&json!({"goal":null})).expect("no goal"));
        assert!(complete_list(&json!({"data":[]})).is_err());
        assert!(complete_list(&json!({"data":[],"nextCursor":"more"})).is_err());
    }

    #[test]
    fn active_goals_and_idle_background_terminals_block_shutdown() {
        let data = json!({"thread":{"status":{"type":"idle"}}});
        let empty = json!({"data":[],"nextCursor":null});
        let goal = parse_thread("test", &data, &json!({"goal":{"status":"active"}}), &empty)
            .expect("goal");
        assert!(!goal.is_idle());
        let terminal = parse_thread(
            "test",
            &data,
            &json!({"goal":null}),
            &json!({"data":[{}],"nextCursor":null}),
        )
        .expect("terminal");
        assert!(!terminal.is_idle());
        assert!(parse_thread("test", &data, &json!({"goal":null}), &empty)
            .expect("idle")
            .is_idle());
    }
}
