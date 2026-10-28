//! Power throttling (plan 1b-4a-p2, R8). Windows lowers a process's quality of
//! service (EcoQoS) by its own heuristics, such as a minimized or background
//! window; on a hybrid CPU with a quiet power plan its threads then run on a
//! few slow cores, and under load waited to run for most of a 40 s span: hash
//! results came back past their deadline and launches went out without a hash.
//! The agent opts out at start, so its timeliness does not depend on them.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetProcessInformation,
};

const SIZE: u32 = size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32;

/// Asks Windows never to throttle `process`'s execution speed (`HighQoS`).
pub fn run_at_full_speed(process: HANDLE) -> windows::core::Result<()> {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE of `SIZE` bytes
    // that outlives the call.
    unsafe { SetProcessInformation(process, ProcessPowerThrottling, (&raw const state).cast(), SIZE) }
}

/// Whether `process` opted out of execution-speed throttling.
pub fn runs_at_full_speed(process: HANDLE) -> windows::core::Result<bool> {
    let mut state =
        PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
    // SAFETY: `state` is a writable PROCESS_POWER_THROTTLING_STATE of `SIZE` bytes.
    unsafe { GetProcessInformation(process, ProcessPowerThrottling, (&raw mut state).cast(), SIZE)? };
    let speed = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
    Ok(state.ControlMask & speed != 0 && state.StateMask & speed == 0)
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn a_process_runs_at_full_speed_once_it_opts_out() {
        // A child, so the test process's own setting is left alone; ping
        // starts no process of its own, so killing it leaves nothing behind.
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn a child");
        let h = HANDLE(child.as_raw_handle());
        let before = runs_at_full_speed(h);
        let eco = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        // SAFETY: as in `run_at_full_speed`.
        let asked = unsafe { SetProcessInformation(h, ProcessPowerThrottling, (&raw const eco).cast(), SIZE) };
        let throttled = runs_at_full_speed(h);
        let set = run_at_full_speed(h);
        let after = runs_at_full_speed(h);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(before.ok(), Some(false), "a new process has opted out of nothing");
        assert!(asked.is_ok() && set.is_ok(), "{asked:?} {set:?}");
        assert_eq!(throttled.ok(), Some(false), "a process that asked to be throttled");
        assert_eq!(after.ok(), Some(true));
    }
}
