//! macOS: makes the playback thread a real-time thread that the scheduler treats as part of
//! the audio device's I/O work (Apple: "Adding Parallel Real-Time Threads to Audio
//! Workgroups"), so Apple Silicon runs it on a performance core with the I/O thread.
//!
//! The thread gets the Mach time-constraint policy for its own period (one process buffer),
//! then joins the device's I/O workgroup (`kAudioDevicePropertyIOThreadOSWorkgroup`),
//! leaving the old one and joining the new one when the device changes.
use std::ffi::c_void;

/// `os_workgroup_join_token_s`: a signature and 36 opaque bytes on 64-bit systems.
#[repr(C)]
struct JoinToken {
    sig: u32,
    opaque: [u8; 36],
}

#[repr(C)]
struct TimeConstraint {
    period: u32,
    computation: u32,
    constraint: u32,
    preemptible: i32,
}

const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;

extern "C" {
    fn pthread_self() -> *mut c_void;
    fn pthread_mach_thread_np(thread: *mut c_void) -> u32;
    fn thread_policy_set(thread: u32, flavor: u32, policy_info: *mut i32, count: u32) -> i32;
    fn os_workgroup_join(wg: *mut c_void, token: *mut JoinToken) -> i32;
    fn os_workgroup_leave(wg: *mut c_void, token: *mut JoinToken);
    fn os_release(object: *mut c_void);
}

/// The playback thread's real-time setup, kept in step with the device and the process
/// buffer.
pub(crate) struct RealTime {
    device: u32,
    workgroup: *mut c_void,
    token: JoinToken,
    joined: bool,
    period: (u32, u32),
    /// Diagnostics: PROWAVE_NO_RT_POLICY / PROWAVE_NO_WORKGROUP leave either step out (read
    /// once, when the thread starts).
    use_policy: bool,
    use_group: bool,
}

impl RealTime {
    pub(crate) fn new() -> Self {
        Self {
            device: 0,
            workgroup: std::ptr::null_mut(),
            token: JoinToken {
                sig: 0,
                opaque: [0; 36],
            },
            joined: false,
            period: (0, 0),
            use_policy: std::env::var_os("PROWAVE_NO_RT_POLICY").is_none(),
            use_group: std::env::var_os("PROWAVE_NO_WORKGROUP").is_none(),
        }
    }

    /// Before each round: the policy for a period of `chunk` frames at `sr`, and the
    /// workgroup of `device` (0: none). Only does work when one of them changed.
    pub(crate) fn follow(&mut self, device: u32, chunk: u32, sr: f32) {
        let sr_key = sr as u32;
        let (policy, group) = (self.use_policy, self.use_group);
        if policy && self.period != (chunk, sr_key) && chunk > 0 && sr_key > 0 {
            self.period = (chunk, sr_key);
            let ns = |frames: f64| (frames * 1e9 / f64::from(sr)) as u64;
            let host = |n: u64| crate::duplex::host_time_from_ns(n).min(u64::from(u32::MAX)) as u32;
            // Real time for one process buffer, of which it may compute half before the
            // scheduler lets others in; it must be done within the period.
            let mut policy = TimeConstraint {
                period: host(ns(f64::from(chunk))),
                computation: host(ns(f64::from(chunk) * 0.5)),
                constraint: host(ns(f64::from(chunk))),
                preemptible: 1,
            };
            // SAFETY: the policy struct matches `thread_time_constraint_policy` and the count
            // its size in integers; the thread is this one.
            let status = unsafe {
                thread_policy_set(
                    pthread_mach_thread_np(pthread_self()),
                    THREAD_TIME_CONSTRAINT_POLICY,
                    (&mut policy as *mut TimeConstraint).cast(),
                    THREAD_TIME_CONSTRAINT_POLICY_COUNT,
                )
            };
            if status != 0 {
                tracing::warn!("playback thread: no real-time policy ({status})");
            }
        }
        if group && device != self.device {
            self.leave();
            self.device = device;
            if device != 0 {
                if let Some(wg) = crate::duplex::io_workgroup(device) {
                    // SAFETY: `wg` is a retained workgroup from the HAL; the token lives in
                    // `self` until `leave`.
                    let status = unsafe { os_workgroup_join(wg, &mut self.token) };
                    self.workgroup = wg;
                    self.joined = status == 0;
                    if status == 0 {
                        tracing::info!("playback thread joined the device's I/O workgroup");
                    } else {
                        tracing::warn!("playback thread: cannot join the I/O workgroup ({status})");
                    }
                }
            }
        }
    }

    fn leave(&mut self) {
        if self.workgroup.is_null() {
            return;
        }
        // SAFETY: the workgroup was joined (when `joined`) on this thread with this token, and
        // is released once.
        unsafe {
            if self.joined {
                os_workgroup_leave(self.workgroup, &mut self.token);
            }
            os_release(self.workgroup);
        }
        self.workgroup = std::ptr::null_mut();
        self.joined = false;
    }
}

impl Drop for RealTime {
    fn drop(&mut self) {
        self.leave();
    }
}
