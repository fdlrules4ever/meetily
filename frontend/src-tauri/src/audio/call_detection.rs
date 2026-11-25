//! Detects calls from other programs using a microphone.
//!
//! Call apps (Teams, Zoom, a browser with Google Meet, Slack huddles, ...) open the microphone
//! for the whole call, also while you are muted, and release it when the call ends. Windows
//! reports this per process through its audio sessions, the same information behind the
//! microphone icon in the taskbar. Meetily's own capture is left out.
//!
//! [`CallEndTracker`] turns those observations into "stop the recording now".

use std::time::Duration;

/// Names of other programs capturing from a microphone right now, sorted and deduplicated.
/// `None` when this cannot be determined (other platforms, or Windows audio errors).
pub fn other_apps_using_microphone() -> Option<Vec<String>> {
    #[cfg(target_os = "windows")]
    {
        match windows_sessions::capturing_process_ids() {
            Ok(pids) => Some(process_names(&pids)),
            Err(e) => {
                log::warn!("Call detection: could not read audio sessions: {}", e);
                None
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn process_names(pids: &[u32]) -> Vec<String> {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pids: Vec<Pid> = pids.iter().map(|pid| Pid::from_u32(*pid)).collect();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), false);
    let mut names: Vec<String> = pids
        .iter()
        .map(|pid| {
            system
                .process(*pid)
                .map(|process| process.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("process {}", pid))
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(target_os = "windows")]
mod windows_sessions {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eCapture, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    /// Balances a successful `CoInitializeEx` on this thread.
    struct ComGuard;

    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    /// Process IDs with an active capture session on any active microphone, except our own.
    pub fn capturing_process_ids() -> windows::core::Result<Vec<u32>> {
        unsafe {
            // S_OK / S_FALSE must be balanced. If the thread already uses another COM mode,
            // COM is still usable and must not be uninitialised here.
            let _com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok().then_some(ComGuard);

            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
            let own_pid = std::process::id();
            let mut pids = Vec::new();
            for device_index in 0..devices.GetCount()? {
                let Ok(device) = devices.Item(device_index) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for session_index in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(session) = sessions.GetSession(session_index) else { continue };
                    if session.GetState().ok() != Some(AudioSessionStateActive) {
                        continue;
                    }
                    let Ok(session) = session.cast::<IAudioSessionControl2>() else { continue };
                    let Ok(pid) = session.GetProcessId() else { continue };
                    if pid != 0 && pid != own_pid && !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
            Ok(pids)
        }
    }
}

/// A call shorter than this (a voice message, a mic test) does not count.
const MIN_CALL: Duration = Duration::from_secs(2 * 60);
/// After the call ends, stop once nobody has spoken for this long...
const QUIET_AFTER_CALL: Duration = Duration::from_secs(60);
/// ...or at the latest this long after the call ended (background chatter can keep the
/// speech detector busy).
const LATEST_AFTER_CALL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallChange {
    Started,
    Ended,
    Rejoined,
}

/// Decides when a recording should stop because the call ended. Times are offsets on any
/// monotonic clock.
#[derive(Debug, Default)]
pub struct CallEndTracker {
    /// When the current (or last) call was first seen.
    started_at: Option<Duration>,
    /// When the microphone was released after a call that lasted long enough.
    ended_at: Option<Duration>,
    active: bool,
}

impl CallEndTracker {
    /// Records whether another app is using a microphone at `now`.
    pub fn observe(&mut self, now: Duration, call_active: bool) -> Option<CallChange> {
        match (self.active, call_active) {
            (false, true) => {
                self.active = true;
                if self.ended_at.take().is_some() {
                    // Back in the call (dropped and rejoined): keep the original start.
                    Some(CallChange::Rejoined)
                } else {
                    self.started_at = Some(now);
                    Some(CallChange::Started)
                }
            }
            (true, false) => {
                self.active = false;
                let lasted = self.started_at.map_or(Duration::ZERO, |start| now.saturating_sub(start));
                if lasted >= MIN_CALL {
                    self.ended_at = Some(now);
                    Some(CallChange::Ended)
                } else {
                    self.started_at = None;
                    None
                }
            }
            _ => None,
        }
    }

    /// True when the call has ended and it has been quiet long enough (or long enough has passed).
    pub fn should_stop(&self, now: Duration, silent_for: Duration) -> bool {
        let Some(ended_at) = self.ended_at else { return false };
        let since_end = now.saturating_sub(ended_at);
        (since_end >= QUIET_AFTER_CALL && silent_for >= QUIET_AFTER_CALL) || since_end >= LATEST_AFTER_CALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn stops_a_minute_after_the_call_once_it_is_quiet() {
        let mut tracker = CallEndTracker::default();
        assert_eq!(tracker.observe(secs(0), true), Some(CallChange::Started));
        assert_eq!(tracker.observe(secs(1800), false), Some(CallChange::Ended));
        assert!(!tracker.should_stop(secs(1830), secs(90)), "too soon after the call");
        assert!(!tracker.should_stop(secs(1860), secs(20)), "someone is still talking");
        assert!(tracker.should_stop(secs(1860), secs(60)));
    }

    #[test]
    fn stops_at_the_latest_ten_minutes_after_the_call() {
        let mut tracker = CallEndTracker::default();
        tracker.observe(secs(0), true);
        tracker.observe(secs(600), false);
        assert!(!tracker.should_stop(secs(600 + 599), secs(5)));
        assert!(tracker.should_stop(secs(600 + 600), secs(5)));
    }

    #[test]
    fn no_call_means_no_stop() {
        let mut tracker = CallEndTracker::default();
        tracker.observe(secs(0), false);
        assert!(!tracker.should_stop(secs(3600), secs(3600)));
    }

    #[test]
    fn short_mic_use_is_not_a_call() {
        let mut tracker = CallEndTracker::default();
        tracker.observe(secs(100), true);
        assert_eq!(tracker.observe(secs(130), false), None);
        assert!(!tracker.should_stop(secs(1000), secs(900)));
    }

    #[test]
    fn rejoining_cancels_the_stop() {
        let mut tracker = CallEndTracker::default();
        tracker.observe(secs(0), true);
        tracker.observe(secs(300), false);
        assert_eq!(tracker.observe(secs(330), true), Some(CallChange::Rejoined));
        assert!(!tracker.should_stop(secs(1000), secs(900)));
        assert_eq!(tracker.observe(secs(1200), false), Some(CallChange::Ended));
        assert!(tracker.should_stop(secs(1260), secs(60)));
    }

    #[test]
    fn a_short_rejoin_after_a_real_call_still_ends_it() {
        let mut tracker = CallEndTracker::default();
        tracker.observe(secs(0), true);
        tracker.observe(secs(600), false);
        // Back for 20 s (the call started 600 s earlier, so it still counts as a call).
        tracker.observe(secs(620), true);
        assert_eq!(tracker.observe(secs(640), false), Some(CallChange::Ended));
        assert!(tracker.should_stop(secs(700), secs(60)));
    }
}
