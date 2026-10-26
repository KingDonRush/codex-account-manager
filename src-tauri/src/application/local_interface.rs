use crate::application::manager::ManagerCore;
use crate::error::AppError;

/// Legacy private metadata/activation must not bypass the v2 consent boundary.
pub fn try_metadata_cli(_identifier: &str) -> bool {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(
        args.first().map(String::as_str),
        Some("--api-v1" | "--activate-profile-v1")
    ) {
        return false;
    }
    println!(
        "{}",
        serde_json::json!({"apiVersion":2,"error":{"code":"consent_required","message":"A interface v1 foi desativada. Use codex-account-manager-cli e autorize a integração na interface."}})
    );
    true
}

pub fn activation_target(args: &[String]) -> Result<Option<&str>, AppError> {
    if args.get(1).map(String::as_str) == Some("--activate-profile-v1") {
        return Err(AppError::ConsentRequired(
            "Ativação externa exige autorização v2.".to_owned(),
        ));
    }
    Ok(None)
}

pub fn activate(_core: &ManagerCore, args: &[String]) {
    if let Err(error) = activation_target(args) {
        eprintln!("{error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn even_valid_legacy_activation_is_refused() {
        assert!(matches!(
            activation_target(&[
                "app".into(),
                "--activate-profile-v1".into(),
                uuid::Uuid::new_v4().to_string()
            ]),
            Err(AppError::ConsentRequired(_))
        ));
        assert_eq!(
            activation_target(&["app".into()]).expect("ordinary open"),
            None
        );
    }
}
