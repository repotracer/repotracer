//! Keep descendants owned even when the direct child has already exited.
use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use tokio::process::{Child, Command};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
        },
    },
};

pub(crate) struct WindowsJob(OwnedHandle);

impl WindowsJob {
    /// Suspend before any provider code executes, assign the job, then resume.
    /// Assigning after a normal spawn would let a fast wrapper escape ownership.
    pub(crate) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let job = Self::new()?;
        command
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        if let Err(error) = job.assign_and_resume(&child) {
            // Also covers failed assignment, where closing the job cannot kill
            // the suspended child yet. Never leave a failed launch suspended.
            let _ = child.start_kill();
            return Err(error);
        }
        Ok((child, job))
    }

    fn new() -> io::Result<Self> {
        // Null security attributes produce a non-inheritable, unnamed handle.
        let job = Self(owned(unsafe {
            CreateJobObjectW(ptr::null(), ptr::null())
        })?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the handle is live and the pointer/size describe `limits`.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        let handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("missing child handle"))?;
        // SAFETY: both handles remain owned for the duration of the call.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("missing child PID"))?;
        resume_initial_thread(pid)
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: callers transfer a newly created handle exactly once.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn resume_initial_thread(pid: u32) -> io::Result<()> {
    // Tokio/std do not expose the initial thread handle. The process was
    // created suspended, so its initial thread cannot have launched helpers.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: the snapshot is live; entry has the required initialized size.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        if entry.th32OwnerProcessID == pid {
            let thread =
                owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: this handle names a thread of the suspended child.
            return match unsafe { ResumeThread(thread.as_raw_handle()) } {
                1 => Ok(()),
                u32::MAX => Err(io::Error::last_os_error()),
                _ => Err(io::Error::other("unexpected child thread suspend count")),
            };
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: as above; neither snapshot nor entry moved out of scope.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    Err(io::Error::other("could not find suspended child thread"))
}
