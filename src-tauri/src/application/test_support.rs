use chrono::{DateTime, Utc};

use crate::domain::ManagedProfile;

pub fn profile(name: &str, now: DateTime<Utc>) -> ManagedProfile {
    serde_json::from_value(serde_json::json!({
        "id": name, "name": name, "accountId": format!("fixture-{name}"),
        "email": format!("{name}@example.test"), "status": "active",
        "credentialState": "ready", "createdAt": now, "updatedAt": now,
    }))
    .expect("fixture profile")
}
