use crate::domain::OperationState;
use crate::error::AppError;

pub fn transition(from: &OperationState, to: OperationState) -> Result<OperationState, AppError> {
    let valid = match from {
        OperationState::Idle => matches!(
            to,
            OperationState::Preparing | OperationState::Cancelled | OperationState::Failed
        ),
        OperationState::Preparing => matches!(
            to,
            OperationState::BrowserLaunching
                | OperationState::Preparing
                | OperationState::Verifying
                | OperationState::Cancelled
                | OperationState::Failed
        ),
        OperationState::BrowserLaunching => matches!(
            to,
            OperationState::AwaitingLogin | OperationState::Cancelled | OperationState::Failed
        ),
        OperationState::AwaitingLogin => matches!(
            to,
            OperationState::Verifying
                | OperationState::Cancelled
                | OperationState::TimedOut
                | OperationState::Failed
        ),
        OperationState::Verifying => matches!(
            to,
            OperationState::AwaitingConfirmation
                | OperationState::Persisting
                | OperationState::Cancelled
                | OperationState::Failed
        ),
        OperationState::AwaitingConfirmation => matches!(
            to,
            OperationState::Persisting
                | OperationState::Cancelled
                | OperationState::Failed
                | OperationState::TimedOut
        ),
        OperationState::Persisting => {
            matches!(to, OperationState::Completed | OperationState::Failed)
        }
        OperationState::Completed
        | OperationState::Cancelled
        | OperationState::Failed
        | OperationState::TimedOut => false,
    };
    if valid {
        Ok(to)
    } else {
        Err(AppError::Internal(format!(
            "transição inválida: {} → {}",
            from.as_str(),
            to.as_str()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::transition;
    use crate::domain::OperationState;

    #[test]
    fn replacement_requires_a_separate_confirmation_before_persisting() {
        let waiting = transition(
            &OperationState::Verifying,
            OperationState::AwaitingConfirmation,
        )
        .expect("confirmation step");
        assert!(transition(&waiting, OperationState::Completed).is_err());
        assert!(transition(&waiting, OperationState::Cancelled).is_ok());
        assert!(transition(&waiting, OperationState::Persisting).is_ok());
    }

    #[test]
    fn accepts_happy_path() {
        let mut state = OperationState::Idle;
        for next in [
            OperationState::Preparing,
            OperationState::BrowserLaunching,
            OperationState::AwaitingLogin,
            OperationState::Verifying,
            OperationState::Persisting,
            OperationState::Completed,
        ] {
            state = transition(&state, next).expect("transição válida");
        }
        assert!(matches!(state, OperationState::Completed));
    }

    #[test]
    fn rejects_skipping_verification() {
        assert!(transition(&OperationState::AwaitingLogin, OperationState::Completed).is_err());
    }

    #[test]
    fn permits_cancellation_while_the_browser_is_waiting() {
        let state = transition(&OperationState::AwaitingLogin, OperationState::Cancelled)
            .expect("cancelamento válido");
        assert!(matches!(state, OperationState::Cancelled));
        assert!(transition(&state, OperationState::Completed).is_err());
    }
}
