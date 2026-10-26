use crate::error::AppError;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;

const MAX_PREFERENCES: u64 = 8 * 1024 * 1024;

/// Managed profiles retain authentication, not the previous operation's tabs.
/// Only an inactive app-owned profile is edited; live manual tabs stay intact.
pub(super) fn prepare_profile_startup(profile: &Path) -> Result<(), AppError> {
    reject_link(profile)?;
    ensure_inactive(profile)?;
    let default = profile.join("Default");
    reject_link(&default)?;
    std::fs::create_dir_all(&default)?;
    let preferences = default.join("Preferences");
    reject_link(&preferences)?;
    let mut value = read_preferences(&preferences)?;
    configure_startup(&mut value)?;
    ensure_inactive(profile)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".startup-")
        .tempfile_in(&default)?;
    serde_json::to_writer(&mut temporary, &value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&preferences)
        .map_err(|error| AppError::Io(error.error))?;
    clear_closed_tabs(&default)?;
    Ok(())
}

fn read_preferences(path: &Path) -> Result<Value, AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_PREFERENCES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PREFERENCES {
        return Err(invalid_preferences());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid_preferences())
}

fn configure_startup(value: &mut Value) -> Result<(), AppError> {
    let root = value.as_object_mut().ok_or_else(invalid_preferences)?;
    let session = root.entry("session").or_insert_with(|| json!({}));
    let session = session.as_object_mut().ok_or_else(invalid_preferences)?;
    // Chromium couples cookie/session-storage recovery to Continue Session (1).
    // Keep that recovery while discarding only the inactive profile's tab ledger.
    // New Tab (5) would drop session cookies even without deleting Cookies.
    session.insert("restore_on_startup".into(), json!(1));
    Ok(())
}

fn clear_closed_tabs(default: &Path) -> Result<(), AppError> {
    let sessions = default.join("Sessions");
    reject_link(&sessions)?;
    let entries = match std::fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let entries = entries.take(101).collect::<Result<Vec<_>, _>>()?;
    if entries.len() > 100 {
        return Err(AppError::Browser(
            "O registro de abas excedeu o limite seguro; nenhum arquivo de abas foi removido."
                .into(),
        ));
    }
    let mut ledgers = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        if name.to_str().is_none_or(|name| !is_tab_ledger(name)) {
            continue;
        }
        reject_link(&entry.path())?;
        if entry.file_type()?.is_file() {
            ledgers.push(entry.path());
        }
    }
    for ledger in ledgers {
        std::fs::remove_file(ledger)?;
    }
    Ok(())
}

fn is_tab_ledger(name: &str) -> bool {
    name.strip_prefix("Session_")
        .or_else(|| name.strip_prefix("Tabs_"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn invalid_preferences() -> AppError {
    AppError::Browser(
        "As preferências do perfil não puderam ser lidas com segurança. O original foi preservado."
            .into(),
    )
}

fn reject_link(path: &Path) -> Result<(), AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if indirect_file(&metadata) => Err(AppError::AccessDenied(
            "Preferências do navegador não podem apontar para fora do perfil gerenciado.".into(),
        )),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

fn indirect_file(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn ensure_inactive(profile: &Path) -> Result<(), AppError> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let active = live_profile_lock(profile, &system)
        || system
            .processes()
            .values()
            .any(|process| uses_profile(process.cmd(), profile));
    if active {
        return Err(AppError::Busy("Este perfil já está aberto fora do controle atual. Nenhuma preferência foi alterada; feche apenas essa janela para continuar.".into()));
    }
    Ok(())
}

fn live_profile_lock(profile: &Path, system: &sysinfo::System) -> bool {
    #[cfg(unix)]
    {
        // Chromium on Linux can rewrite argv into one process-title string.
        // Its profile singleton still identifies the live owner. Never remove
        // that lock or edit underneath it; an ambiguous live PID is busy too.
        std::fs::read_link(profile.join("SingletonLock"))
            .ok()
            .and_then(|lock| {
                lock.to_str()
                    .and_then(|text| text.rsplit_once('-').map(|(_, pid)| pid))
                    .and_then(|pid| pid.parse::<u32>().ok())
            })
            .is_some_and(|pid| system.process(sysinfo::Pid::from_u32(pid)).is_some())
    }
    #[cfg(not(unix))]
    {
        let _ = (profile, system);
        false
    }
}

fn uses_profile(arguments: &[std::ffi::OsString], profile: &Path) -> bool {
    arguments.iter().enumerate().any(|(index, argument)| {
        let text = argument.to_string_lossy();
        let path = text
            .strip_prefix("--user-data-dir=")
            .map(Path::new)
            .or_else(|| {
                (text == "--user-data-dir")
                    .then(|| arguments.get(index + 1).map(Path::new))
                    .flatten()
            });
        path.is_some_and(|path| {
            path == profile
                || std::fs::canonicalize(path)
                    .ok()
                    .zip(std::fs::canonicalize(profile).ok())
                    .is_some_and(|(left, right)| left == right)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_changes_only_the_restore_choice_and_preserves_authentication_files() {
        let dir = tempfile::tempdir().expect("fixture");
        let default = dir.path().join("Default");
        std::fs::create_dir(&default).expect("profile");
        let original = json!({"session":{"restore_on_startup":5,"startup_urls":["https://example.test/"]},"profile":{"exit_type":"Crashed"},"other":{"setting":true}});
        std::fs::write(
            default.join("Preferences"),
            serde_json::to_vec(&original).expect("json"),
        )
        .expect("preferences");
        std::fs::write(default.join("Cookies"), b"synthetic-cookie-store").expect("cookies");
        prepare_profile_startup(dir.path()).expect("prepare");
        let saved = read_preferences(&default.join("Preferences")).expect("read");
        let mut expected = original;
        expected["session"]["restore_on_startup"] = json!(1);
        assert_eq!(saved, expected);
        assert_eq!(
            std::fs::read(default.join("Cookies")).expect("cookies"),
            b"synthetic-cookie-store"
        );
    }
    #[test]
    fn malformed_preferences_are_preserved_instead_of_resetting_the_profile() {
        let dir = tempfile::tempdir().expect("fixture");
        let default = dir.path().join("Default");
        std::fs::create_dir(&default).expect("profile");
        for original in [b"{broken".as_slice(), b"[]", b"{\"session\":false}"] {
            std::fs::write(default.join("Preferences"), original).expect("preferences");
            assert!(prepare_profile_startup(dir.path()).is_err());
            assert_eq!(
                std::fs::read(default.join("Preferences")).expect("original"),
                original
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn preferences_link_cannot_overwrite_a_file_outside_the_profile() {
        let dir = tempfile::tempdir().expect("fixture");
        let default = dir.path().join("Default");
        std::fs::create_dir(&default).expect("profile");
        let original = dir.path().join("outside.json");
        std::fs::write(&original, b"{}").expect("outside");
        std::os::unix::fs::symlink(&original, default.join("Preferences")).expect("link");
        assert!(prepare_profile_startup(dir.path()).is_err());
        assert_eq!(std::fs::read(original).expect("untouched"), b"{}");
    }
    #[test]
    fn live_profile_detection_uses_an_exact_path_not_a_prefix() {
        let dir = tempfile::tempdir().expect("fixture");
        let profile = dir.path().join("profile");
        assert!(uses_profile(
            &[format!("--user-data-dir={}", profile.display()).into()],
            &profile
        ));
        assert!(uses_profile(
            &["--user-data-dir".into(), profile.as_os_str().into()],
            &profile
        ));
        assert!(!uses_profile(
            &[format!("--user-data-dir={}2", profile.display()).into()],
            &profile
        ));
    }

    #[test]
    fn cleanup_removes_only_the_closed_tab_ledger_not_cookie_or_storage_data() {
        let dir = tempfile::tempdir().expect("fixture");
        let default = dir.path().join("Default");
        let sessions = default.join("Sessions");
        std::fs::create_dir_all(&sessions).expect("session directory");
        for name in ["Session_123", "Tabs_456", "unknown-file", "Session_cookie"] {
            std::fs::write(sessions.join(name), b"synthetic").expect("fixture file");
        }
        std::fs::write(default.join("Cookies"), b"cookie-store").expect("cookies");
        std::fs::create_dir(default.join("Session Storage")).expect("storage");
        std::fs::write(default.join("Session Storage/fixture"), b"local-state")
            .expect("local state");
        prepare_profile_startup(dir.path()).expect("startup");
        assert!(!sessions.join("Session_123").exists());
        assert!(!sessions.join("Tabs_456").exists());
        assert!(sessions.join("unknown-file").exists());
        assert!(sessions.join("Session_cookie").exists());
        assert_eq!(
            std::fs::read(default.join("Cookies")).expect("cookies"),
            b"cookie-store"
        );
        assert_eq!(
            std::fs::read(default.join("Session Storage/fixture")).expect("storage"),
            b"local-state"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_an_indirect_ledger_without_deleting_other_candidates() {
        let dir = tempfile::tempdir().expect("fixture");
        let default = dir.path().join("Default");
        let sessions = default.join("Sessions");
        std::fs::create_dir_all(&sessions).expect("sessions");
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"preserve").expect("outside");
        std::fs::write(sessions.join("Session_1"), b"ledger").expect("ledger");
        std::os::unix::fs::symlink(&outside, sessions.join("Tabs_2")).expect("link");
        assert!(clear_closed_tabs(&default).is_err());
        assert!(sessions.join("Session_1").exists());
        assert_eq!(std::fs::read(outside).expect("outside"), b"preserve");
    }
}
