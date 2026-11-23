//! Small platform shims so the rest of the app stays free of `cfg` noise.

use std::process::{Command, Stdio};

/// Keep child processes from flashing a console window on Windows.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn open_url(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match no_window(&mut cmd).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Windows: make sure `child` is killed when drift exits, even if drift
/// crashes (the equivalent of Linux's PR_SET_PDEATHSIG).
#[cfg(windows)]
pub fn kill_on_exit(child: &std::process::Child) {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    #[repr(C)]
    struct BasicLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    struct IoCounters([u64; 6]);
    #[repr(C)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *mut c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    }

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    // The job handle is intentionally kept open for the life of the process;
    // when drift exits (or dies) Windows closes it and kills everything in it.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        let mut info: ExtendedLimits = std::mem::zeroed();
        info.basic.limit_flags = KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ExtendedLimits>() as u32,
        );
        if ok == 0 {
            return 0;
        }
        job as usize
    });
    if job != 0 {
        unsafe {
            AssignProcessToJobObject(job as *mut c_void, child.as_raw_handle() as *mut c_void);
        }
    }
}

#[cfg(not(windows))]
pub fn kill_on_exit(_child: &std::process::Child) {}
