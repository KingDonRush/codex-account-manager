use std::path::PathBuf;

use windows::core::{Owned, HSTRING, PCWSTR};
use windows::Data::Xml::Dom::{XmlDocument, XmlLoadSettings};
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, AO_NONE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

use super::installation::{package_executable, DesktopInstallation};
pub(super) use super::process_identity::{created_ticks, same_session};
use crate::error::AppError;

// Package family is stable across Store updates, unlike its versioned directory.
const CODEX_PACKAGE_FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";

struct Apartment(bool);

impl Apartment {
    fn enter() -> Result<Self, AppError> {
        // SAFETY: initialization is paired on this same thread; an existing STA
        // is reused without changing or uninitializing its owner's apartment.
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self(false)),
            Err(error) => Err(discovery_error(error)),
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: this thread owns one successful initialization above.
            unsafe { RoUninitialize() };
        }
    }
}

pub(super) fn discover_packaged() -> Result<Option<DesktopInstallation>, AppError> {
    let _apartment = Apartment::enter()?;
    let manager = PackageManager::new().map_err(discovery_error)?;
    let packages = manager
        .FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(),
            &HSTRING::from(CODEX_PACKAGE_FAMILY),
        )
        .map_err(discovery_error)?;
    let iterator = packages.First().map_err(discovery_error)?;
    if !iterator.HasCurrent().map_err(discovery_error)? {
        return Ok(None);
    }
    let package = iterator.Current().map_err(discovery_error)?;
    if iterator.MoveNext().map_err(discovery_error)? {
        return Err(AppError::DesktopDiscovery(
            "Há mais de uma instalação registrada do Codex; escolha a instalação antes de trocar."
                .to_owned(),
        ));
    }
    let root = package
        .InstalledLocation()
        .and_then(|folder| folder.Path())
        .map_err(discovery_error)?;
    installation_from_manifest(PathBuf::from(root.to_string_lossy())).map(Some)
}

fn installation_from_manifest(root: PathBuf) -> Result<DesktopInstallation, AppError> {
    use std::io::Read;
    const MAX_MANIFEST: u64 = 1024 * 1024;
    let mut content = String::new();
    std::fs::File::open(root.join("AppxManifest.xml"))?
        .take(MAX_MANIFEST + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_MANIFEST {
        return Err(AppError::DesktopDiscovery(
            "Manifesto do Codex excede o limite de leitura.".to_owned(),
        ));
    }
    let document = XmlDocument::new().map_err(discovery_error)?;
    let settings = XmlLoadSettings::new().map_err(discovery_error)?;
    settings.SetProhibitDtd(true).map_err(discovery_error)?;
    document
        .LoadXmlWithSettings(
            &HSTRING::from(content.trim_start_matches('\u{feff}')),
            &settings,
        )
        .map_err(discovery_error)?;
    let applications = document
        .SelectNodes(&HSTRING::from("//*[local-name()='Application']"))
        .map_err(discovery_error)?;
    let mut found = None;
    for index in 0..applications.Length().map_err(discovery_error)?.min(32) {
        let attributes = applications
            .Item(index)
            .and_then(|node| node.Attributes())
            .map_err(discovery_error)?;
        let executable = attributes
            .GetNamedItem(&HSTRING::from("Executable"))
            .and_then(|node| node.InnerText())
            .map_err(discovery_error)?
            .to_string_lossy();
        let name = std::path::Path::new(&executable)
            .file_name()
            .and_then(|name| name.to_str());
        if !matches!(name, Some("ChatGPT.exe" | "Codex.exe")) {
            continue;
        }
        let id = attributes
            .GetNamedItem(&HSTRING::from("Id"))
            .and_then(|node| node.InnerText())
            .map_err(discovery_error)?;
        let installation = DesktopInstallation {
            executable: package_executable(&root, &executable)?,
            source: "store",
            app_user_model_id: Some(format!("{CODEX_PACKAGE_FAMILY}!{id}")),
        };
        if found.replace(installation).is_some() {
            return Err(AppError::DesktopDiscovery(
                "O pacote possui mais de um Desktop elegível; escolha explícita necessária."
                    .to_owned(),
            ));
        }
    }
    found.ok_or_else(|| {
        AppError::DesktopDiscovery(
            "O manifesto registrado não identifica o executável do Codex.".to_owned(),
        )
    })
}

pub(super) fn activate(app_id: &str) -> Result<u32, AppError> {
    let _apartment = Apartment::enter()?;
    // SAFETY: a documented COM class/interface; no aggregation or external argv.
    let launcher: IApplicationActivationManager =
        unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }
            .map_err(|error| AppError::DesktopLaunch(error.to_string()))?;
    let id = HSTRING::from(app_id);
    // SAFETY: id remains alive, null arguments, normal activation in this session.
    unsafe { launcher.ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR::null(), AO_NONE) }
        .map_err(|error| AppError::DesktopLaunch(error.to_string()))
}

struct WindowQuery {
    pid: u32,
    windows: Vec<HWND>,
}

unsafe extern "system" fn collect_window(hwnd: HWND, context: LPARAM) -> windows::core::BOOL {
    // SAFETY: EnumWindows is synchronous; context points to a live WindowQuery.
    let query = unsafe { &mut *(context.0 as *mut WindowQuery) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == query.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        query.windows.push(hwnd);
    }
    windows::core::BOOL(1)
}

fn visible_windows(pid: u32) -> Result<Vec<HWND>, AppError> {
    let mut query = WindowQuery {
        pid,
        windows: Vec::new(),
    };
    // SAFETY: callback/context live for the synchronous enumeration only.
    unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(std::ptr::addr_of_mut!(query) as isize),
        )
    }
    .map_err(|error| AppError::DesktopClosePending(error.to_string()))?;
    Ok(query.windows)
}

pub(super) fn has_window(pid: u32) -> bool {
    visible_windows(pid).is_ok_and(|windows| !windows.is_empty())
}

fn discovery_error(error: windows::core::Error) -> AppError {
    AppError::DesktopDiscovery(format!("Consulta nativa Windows: {error}"))
}

pub(super) fn belongs_to_codex_package(pid: u32) -> Result<bool, AppError> {
    use windows::Win32::Foundation::{
        APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
    };
    use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
    // SAFETY: query-only handle; package identity remains metadata, not argv.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(discovery_error)?;
    let process = unsafe { Owned::new(handle) };
    let mut length = 0;
    let status = unsafe { GetPackageFamilyName(*process, &mut length, None) };
    if status == APPMODEL_ERROR_NO_PACKAGE {
        return Ok(false);
    }
    if status != ERROR_INSUFFICIENT_BUFFER || length > 1024 {
        return Err(AppError::DesktopDiscovery(
            "Não consegui confirmar a identidade do pacote do processo.".to_owned(),
        ));
    }
    let mut buffer = vec![0u16; length as usize];
    let status = unsafe {
        GetPackageFamilyName(
            *process,
            &mut length,
            Some(windows::core::PWSTR(buffer.as_mut_ptr())),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(AppError::DesktopDiscovery(
            "A identidade do processo mudou durante a consulta.".to_owned(),
        ));
    }
    Ok(
        String::from_utf16_lossy(&buffer[..length.saturating_sub(1) as usize])
            == CODEX_PACKAGE_FAMILY,
    )
}

pub(super) fn confirm_interruption(owner: isize) -> Result<bool, AppError> {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
    };
    // SAFETY: trusted parent HWND, fixed text, default No. This decision exists
    // only in a native dialog, not an externally invocable auto-confirm endpoint.
    let result = unsafe {
        MessageBoxW(Some(HWND(owner as *mut _)), w!("O Codex não concluiu o fechamento normal. Encerrar agora pode interromper todos os chats e trabalhos deste Desktop.\n\nA conta não será trocada por este atalho. Deseja encerrar o Codex identificado para tentar novamente?"), w!("Confirmar interrupção do Codex"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
    };
    if result.0 == 0 {
        return Err(AppError::DesktopClosePending(
            "A confirmação nativa não pôde ser aberta.".to_owned(),
        ));
    }
    Ok(result == IDYES)
}

pub(super) fn terminate_verified(pid: u32, expected: Option<u64>) -> bool {
    use windows::Win32::System::Threading::{TerminateProcess, PROCESS_TERMINATE};
    let Some(expected) = expected else {
        return false;
    };
    // SAFETY: only the verified target is opened; holding its handle prevents
    // a recycled PID from substituting another process between check and kill.
    let handle = match unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            false,
            pid,
        )
    } {
        Ok(handle) => unsafe { Owned::new(handle) },
        Err(error) if error.code().0 == 0x8007_0057_u32 as i32 => return true,
        Err(_) => return false,
    };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(*handle, &mut created, &mut exited, &mut kernel, &mut user) }
        .is_err()
    {
        return false;
    }
    let actual = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if actual != expected {
        return true;
    }
    unsafe { TerminateProcess(*handle, 1) }.is_ok()
}

pub(super) fn pick_executable(owner: isize) -> Result<Option<PathBuf>, AppError> {
    // File dialogs require an STA. Do not change a reused blocking worker's
    // apartment, or inherit the package discovery's MTA initialization.
    std::thread::Builder::new()
        .name("desktop-file-picker".to_owned())
        .spawn(move || pick_on_sta(owner))?
        .join()
        .map_err(|_| {
            AppError::DesktopDiscovery("O seletor nativo encerrou inesperadamente.".to_owned())
        })?
}

fn pick_on_sta(owner: isize) -> Result<Option<PathBuf>, AppError> {
    use windows::core::w;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST, SIGDN_FILESYSPATH,
    };
    // SAFETY: this dedicated thread has no existing apartment; balanced below.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(discovery_error)?;
    struct Sta;
    impl Drop for Sta {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _sta = Sta;
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(discovery_error)?;
    let filters = [COMDLG_FILTERSPEC {
        pszName: w!("Codex Desktop"),
        pszSpec: w!("ChatGPT.exe;Codex.exe"),
    }];
    // SAFETY: filter strings are static; parent HWND comes from the trusted app
    // window, never from a client-supplied path or external integration.
    unsafe { dialog.SetFileTypes(&filters) }.map_err(discovery_error)?;
    unsafe { dialog.SetOptions(dialog.GetOptions().map_err(discovery_error)? | FOS_FILEMUSTEXIST) }
        .map_err(discovery_error)?;
    if let Err(error) = unsafe { dialog.Show(Some(HWND(owner as *mut _))) } {
        if error.code().0 == 0x8007_04C7_u32 as i32 {
            return Ok(None);
        }
        return Err(discovery_error(error));
    }
    let result = unsafe { dialog.GetResult() }.map_err(discovery_error)?;
    let text = unsafe { result.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(discovery_error)?;
    let path = unsafe { text.to_string() };
    // SAFETY: GetDisplayName allocates with CoTaskMemAlloc, freed exactly once.
    unsafe { CoTaskMemFree(Some(text.0.cast())) };
    let path = PathBuf::from(path.map_err(|error| AppError::DesktopDiscovery(error.to_string()))?);
    if !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("ChatGPT.exe" | "Codex.exe")
    ) {
        return Err(AppError::DesktopDiscovery(
            "Selecione o executável do Desktop, não a CLI.".to_owned(),
        ));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads only installed package metadata; requires Windows with registered Codex Store package"]
    fn installed_store_package_has_an_executable_and_activation_identity() {
        let installation = discover_packaged()
            .expect("native discovery")
            .expect("registered package");
        assert!(installation.executable.is_file());
        assert!(installation
            .app_user_model_id
            .as_deref()
            .is_some_and(|id| id.starts_with("OpenAI.Codex_2p2nqsd0c76g0!")));
        assert_eq!(installation.source, "store");
    }
}
