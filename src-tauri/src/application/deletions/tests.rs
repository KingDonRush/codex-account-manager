use super::*;
fn request(id: &str) -> DeletionRequest {
    DeletionRequest {
        id: id.to_owned(),
        requester: "fixture".to_owned(),
        grant_id: None,
        items: Vec::new(),
        state: DeletionState::AwaitingConfirmation,
        deleted_ids: Vec::new(),
        error_code: None,
    }
}
#[test]
fn adding_request_keeps_running_confirmation_and_its_receipt() {
    let book = DeletionBook::default();
    book.add(request("first")).expect("add");
    let mut running = book.begin("first").expect("begin");
    book.add(request("second")).expect("add second");
    assert_eq!(
        book.get("first").expect("still running").state,
        DeletionState::Confirming
    );
    assert!(book.deny("first").is_err());
    running.state = DeletionState::Completed;
    book.save(running);
    book.add(request("third")).expect("add third");
    assert_eq!(
        book.get("first").expect("receipt").state,
        DeletionState::Completed
    );
}
#[test]
fn limit_counts_running_confirmations_and_retains_recent_receipts() {
    let book = DeletionBook::default();
    for index in 0..8 {
        book.add(request(&index.to_string())).expect("add");
    }
    book.begin("0").expect("begin");
    assert!(book.add(request("overflow")).is_err());
    for index in 1..8 {
        book.deny(&index.to_string()).expect("deny");
    }
    for index in 8..80 {
        let id = index.to_string();
        book.add(request(&id)).expect("bounded add");
        book.deny(&id).expect("deny");
    }
    assert_eq!(book.0.lock().requests.len(), 64);
    assert_eq!(
        book.get("0").expect("not evicted").state,
        DeletionState::Confirming
    );
    assert_eq!(
        book.get("79").expect("last receipt").state,
        DeletionState::Denied
    );
}
