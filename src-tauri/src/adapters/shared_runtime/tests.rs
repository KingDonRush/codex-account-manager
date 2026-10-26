use super::*;

#[test]
fn interruption_consent_does_not_authorize_a_replacement_server_or_new_threads() {
    let owner = DaemonIdentity {
        pid: 101,
        started_ticks: 25,
        executable: PathBuf::new(),
        #[cfg(unix)]
        socket: PathBuf::new(),
    };
    let mut snapshot = SharedRuntimeSnapshot::absent();
    snapshot.threads = vec![RuntimeThread {
        id: "a".into(),
        name: None,
        state: "active".into(),
        active_goal: false,
        background_terminals: 0,
    }];
    let mut consent = RuntimeConsent {
        pid: 101,
        started_ticks: 25,
        thread_ids: vec!["a".into()],
    };
    assert!(consent_matches(&consent, &owner, &snapshot));
    consent.started_ticks += 1;
    assert!(!consent_matches(&consent, &owner, &snapshot));
    consent.started_ticks = 25;
    snapshot.threads[0].id = "new-chat".into();
    assert!(!consent_matches(&consent, &owner, &snapshot));
}
