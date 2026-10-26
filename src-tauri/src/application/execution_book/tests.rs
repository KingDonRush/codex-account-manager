use super::*;
use std::collections::BTreeSet;
fn job(id: &str, app: &str) -> ExecutionStatus {
    ExecutionStatus {
        id: id.to_owned(),
        application_id: app.to_owned(),
        grant_id: "grant".to_owned(),
        binding_id: "binding".to_owned(),
        binding_revision: 1,
        group_id: None,
        account_id: None,
        state: ExecutionState::Queued,
        started_at: chrono::Utc::now(),
        finished_at: None,
        error_code: None,
        exit_code: None,
        background: false,
    }
}
fn add(book: &ExecutionBook, id: &str, app: &str, accounts: &[&str]) {
    book.add(
        job(id, app),
        Arc::new(AtomicBool::new(false)),
        accounts.iter().map(|id| (*id).to_owned()).collect(),
    )
    .expect("add");
}
#[test]
fn unrelated_accounts_run_in_parallel_while_shared_session_waits() {
    let book = Arc::new(ExecutionBook::default());
    add(&book, "A1", "app-a", &["A"]);
    add(&book, "A2", "app-a", &["A"]);
    add(&book, "B", "app-b", &["B"]);
    let first = book.try_reserve("A1", "A", "session-a", 1).expect("A free");
    assert!(book.try_reserve("A2", "A", "session-a", 2).is_none());
    let second = book
        .try_reserve("B", "B", "session-b", 1)
        .expect("independent B");
    drop(first);
    assert!(book.try_reserve("A2", "A", "session-a", 1).is_some());
    drop(second);
}
#[test]
fn competing_apps_alternate_and_keep_their_own_order() {
    let book = Arc::new(ExecutionBook::default());
    add(&book, "A1", "app-a", &["A"]);
    add(&book, "A2", "app-a", &["A"]);
    add(&book, "B1", "app-b", &["A"]);
    drop(book.try_reserve("A1", "A", "s", 1).expect("first"));
    assert!(book.try_reserve("A2", "A", "s", 1).is_none());
    drop(book.try_reserve("B1", "A", "s", 1).expect("other app"));
    assert!(book.try_reserve("A2", "A", "s", 1).is_some());
}
#[test]
fn group_membership_never_duplicates_an_account_capacity() {
    let book = Arc::new(ExecutionBook::default());
    add(&book, "G1", "app-a", &["A"]);
    add(&book, "G2", "app-b", &["A"]);
    let lease = book.try_reserve("G1", "A", "s1", 1).expect("first group");
    assert!(book.try_reserve("G2", "A", "s2", 1).is_none());
    drop(lease);
    assert!(book.try_reserve("G2", "A", "s2", 1).is_some());
}
#[test]
fn cancellation_of_one_app_does_not_affect_other_work() {
    let book = ExecutionBook::default();
    add(&book, "A", "app-a", &["account"]);
    add(&book, "B", "app-b", &["account"]);
    book.cancel_matching(|job| job.application_id == "app-a");
    assert_eq!(
        book.status("A").expect("A").state,
        ExecutionState::Cancelling
    );
    assert_eq!(book.status("B").expect("B").state, ExecutionState::Queued);
    let _ = BTreeSet::<String>::new();
}

#[test]
fn cancelled_job_neither_starts_nor_blocks_the_next_application() {
    let book = Arc::new(ExecutionBook::default());
    add(&book, "A", "app-a", &["account"]);
    add(&book, "B", "app-b", &["account"]);
    book.cancel("A").expect("cancel");
    assert!(book.try_reserve("A", "account", "session", 1).is_none());
    let lease = book
        .try_reserve("B", "account", "session", 1)
        .expect("not blocked by cancelled A");
    assert!(book.account_busy("account"));
    assert_eq!(
        book.status("B")
            .expect("chosen identity")
            .account_id
            .as_deref(),
        Some("account")
    );
    drop(lease);
    assert!(!book.account_busy("account"));
}

#[test]
fn metadata_failure_removes_workerless_submission_from_fair_queue() {
    let book = Arc::new(ExecutionBook::default());
    add(&book, "A", "app-a", &["account"]);
    add(&book, "B", "app-b", &["account"]);
    book.fail_submission(
        "A",
        &AppError::Internal("synthetic metadata failure".to_owned()),
    );
    assert_eq!(
        book.status("A").expect("failure receipt").state,
        ExecutionState::Failed
    );
    assert!(book.try_reserve("B", "account", "session", 1).is_some());
}
