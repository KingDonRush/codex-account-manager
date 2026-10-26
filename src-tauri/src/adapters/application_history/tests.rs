use super::*;

#[test]
fn export_contains_only_events_and_rollouts_never_auth_or_other_session() {
    let dir = tempfile::tempdir().expect("fixture");
    let id = uuid::Uuid::new_v4().to_string();
    let directory = session_directory(dir.path(), &id).expect("directory");
    let rollouts = directory.join("runtime/sessions/2026");
    std::fs::create_dir_all(&rollouts).expect("rollouts");
    std::fs::write(directory.join("events.jsonl"), b"{\"fixture\":true}\n").expect("events");
    std::fs::write(
        rollouts.join("rollout.jsonl"),
        b"{\"private-thread\":true}\n",
    )
    .expect("rollout");
    std::fs::write(
        directory.join("runtime/auth.json"),
        b"never-export-credential",
    )
    .expect("auth fixture");
    let export_id = uuid::Uuid::new_v4().to_string();
    let path = export(dir.path(), &id, &export_id).expect("archive");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).expect("file")).expect("zip");
    assert_eq!(archive.len(), 2);
    assert!(archive.by_name("events.jsonl").is_ok());
    assert!(archive
        .by_name("runtime/sessions/2026/rollout.jsonl")
        .is_ok());
    assert!(archive.by_name("runtime/auth.json").is_err());
    let neighbor = uuid::Uuid::new_v4().to_string();
    let neighbor_dir = session_directory(dir.path(), &neighbor).expect("neighbor");
    std::fs::create_dir_all(&neighbor_dir).expect("create neighbor");
    clear(dir.path(), &id).expect("clear exact session");
    assert!(neighbor_dir.exists());
    assert!(!directory.exists());
    assert!(clear(dir.path(), "../../outside").is_err());
}
#[cfg(unix)]
#[test]
fn symlink_never_redirects_export_or_clear_to_another_directory() {
    let dir = tempfile::tempdir().expect("fixture");
    let outside = tempfile::tempdir().expect("outside");
    let id = uuid::Uuid::new_v4().to_string();
    let directory = session_directory(dir.path(), &id).expect("directory");
    std::fs::create_dir_all(directory.parent().expect("root")).expect("root");
    std::os::unix::fs::symlink(outside.path(), &directory).expect("link fixture");
    assert!(clear(dir.path(), &id).is_err());
    assert!(export(dir.path(), &id, &uuid::Uuid::new_v4().to_string()).is_err());
    assert!(outside.path().exists());
}
#[test]
fn archive_has_private_permissions_and_rejects_replacing_an_existing_export() {
    let dir = tempfile::tempdir().expect("fixture");
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(session_directory(dir.path(), &id).expect("session"))
        .expect("directory");
    let export_id = uuid::Uuid::new_v4().to_string();
    let path = export(dir.path(), &id, &export_id).expect("export");
    assert!(path.is_file());
    assert!(export(dir.path(), &id, &export_id).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path)
                .expect("permissions")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
