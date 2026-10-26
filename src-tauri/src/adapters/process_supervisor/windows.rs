use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows::core::Owned;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

use crate::error::AppError;

pub(super) struct WindowsJob(Owned<HANDLE>);

// SAFETY: Windows job handles support cross-thread use; this owner never mutates
// the handle or exposes it, and closes it once after its last Rust owner leaves.
unsafe impl Send for WindowsJob {}
// SAFETY: TerminateJobObject is thread-safe and the owned handle outlives callers.
unsafe impl Sync for WindowsJob {}

impl WindowsJob {
    pub(super) fn attach(child: &Child) -> Result<Self, AppError> {
        let job = Self::create()?;
        let process = HANDLE(child.as_raw_handle());
        // SAFETY: child retains its process handle and job is a live owned handle.
        unsafe { AssignProcessToJobObject(*job.0, process) }.map_err(native_error)?;
        resume_primary_thread(child.id())?;
        Ok(job)
    }

    fn create() -> Result<Self, AppError> {
        // SAFETY: default security, unnamed job; success returns an owned handle.
        let handle = unsafe { CreateJobObjectW(None, None) }.map_err(native_error)?;
        let job = Self(unsafe { Owned::new(handle) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits has the exact ABI/layout and stays alive during the call.
        unsafe {
            SetInformationJobObject(
                *job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(native_error)?;
        Ok(job)
    }

    pub(super) fn terminate(&self) -> Result<(), AppError> {
        // SAFETY: the owner retains this job handle until the call returns.
        unsafe { TerminateJobObject(*self.0, 1) }.map_err(native_error)
    }
}

fn resume_primary_thread(pid: u32) -> Result<(), AppError> {
    // The process was born suspended, so it cannot create additional threads or
    // descendants before this snapshot and assignment to the kill-on-close job.
    let thread_id = primary_thread_id(pid)?;
    // SAFETY: only resume permission is requested for the identified thread.
    let handle =
        unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, thread_id) }.map_err(native_error)?;
    let thread = unsafe { Owned::new(handle) };
    // SAFETY: this live handle refers to the sole suspended thread of our child.
    if unsafe { ResumeThread(*thread) } == u32::MAX {
        return Err(native_error(windows::core::Error::from_win32()));
    }
    Ok(())
}

fn primary_thread_id(pid: u32) -> Result<u32, AppError> {
    // SAFETY: thread snapshots have no pointer arguments and return owned handles.
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(native_error)?;
    let snapshot = unsafe { Owned::new(handle) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is initialized with its ABI size and remains writable.
    unsafe { Thread32First(*snapshot, &mut entry) }.map_err(native_error)?;
    loop {
        if entry.th32OwnerProcessID == pid {
            return Ok(entry.th32ThreadID);
        }
        if unsafe { Thread32Next(*snapshot, &mut entry) }.is_err() {
            return Err(AppError::Runtime(
                "A thread inicial do auxiliar não foi encontrada; lançamento cancelado.".to_owned(),
            ));
        }
    }
}

fn native_error(error: windows::core::Error) -> AppError {
    AppError::Runtime(format!("Supervisão Windows: {error}"))
}
