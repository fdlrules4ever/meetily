//! Watches a running recording, every few seconds:
//!
//! - "Still recording?" reminder. The recording pipeline already runs VAD over the mixed
//!   microphone + call audio, and calls [`mark_speech`] for every window in which it hears
//!   speech. Once it has been quiet on both sides for the minutes set in Recording Settings, a
//!   notification is shown, then again every [`REPEAT_EVERY`] while it stays quiet. Speech
//!   resets it; paused time does not count. Because it listens for speech rather than
//!   loudness, keyboard or fan noise does not reset it.
//! - Stop when the call ends (Windows): when the call app releases the microphone and it is
//!   quiet, the recording is stopped like the tray's Stop does. See `call_detection`.
//! - The tray tooltip's recording time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use log::{info, warn};
use tauri::{AppHandle, Runtime};

use crate::audio::call_detection::{other_apps_using_microphone, CallEndTracker};
use crate::notifications::types::{Notification, NotificationType};

const CHECK_EVERY: Duration = Duration::from_secs(5);
/// After the first reminder, remind again this often while it stays quiet.
const REPEAT_EVERY: Duration = Duration::from_secs(10 * 60);

/// Milliseconds on [`now_ms`]'s clock at which speech was last heard.
static LAST_SPEECH_MS: AtomicU64 = AtomicU64::new(0);
/// Bumped for every recording so a watcher from an earlier recording stops.
static WATCH_GENERATION: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Called by the recording pipeline whenever the VAD hears speech (mic or call audio).
pub fn mark_speech() {
    LAST_SPEECH_MS.store(now_ms(), Ordering::Relaxed);
}

fn silence() -> Duration {
    Duration::from_millis(now_ms().saturating_sub(LAST_SPEECH_MS.load(Ordering::Relaxed)))
}

/// Decides when to remind, from how long it has been quiet.
#[derive(Debug)]
struct ReminderSchedule {
    threshold: Duration,
    repeat: Duration,
    /// Length of the silence at the last reminder of the current quiet stretch.
    reminded_at: Option<Duration>,
}

impl ReminderSchedule {
    fn new(threshold: Duration, repeat: Duration) -> Self {
        Self { threshold, repeat, reminded_at: None }
    }

    /// True when a reminder is due for a stretch that has been quiet for `silent_for`.
    fn due(&mut self, silent_for: Duration) -> bool {
        if silent_for < self.threshold {
            // Someone spoke: the next quiet stretch starts over.
            self.reminded_at = None;
            return false;
        }
        let due = match self.reminded_at {
            None => true,
            Some(at) => silent_for >= at + self.repeat,
        };
        if due {
            self.reminded_at = Some(silent_for);
        }
        due
    }

    fn reset(&mut self) {
        self.reminded_at = None;
    }
}

/// "1 minute", "12 minutes", "2 h 5 min".
fn describe(silent_for: Duration) -> String {
    let minutes = (silent_for.as_secs() / 60).max(1);
    match minutes {
        1 => "1 minute".to_string(),
        m if m < 60 => format!("{} minutes", m),
        m if m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{} h {} min", m / 60, m % 60),
    }
}

fn reminder_notification(silent_for: Duration) -> Notification {
    Notification::new(
        "Meetily is still recording",
        format!(
            "No speech for {}. Stop the recording if the meeting is over.",
            describe(silent_for)
        ),
        NotificationType::SilenceReminder,
    )
}

fn call_ended_notification() -> Notification {
    Notification::new(
        "Recording stopped",
        "Meetily stopped the recording because the call ended. You can turn this off in Recording Settings.",
        NotificationType::CallEndedStop,
    )
}

/// Starts the watcher for a recording that has just started. Replaces any earlier watcher.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let generation = WATCH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    // The first quiet stretch starts now.
    mark_speech();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Read once per recording; a change applies from the next recording.
        let prefs = match crate::audio::recording_preferences::load_recording_preferences(&app).await {
            Ok(prefs) => prefs,
            Err(e) => {
                warn!("Recording watch: could not load recording preferences, using the defaults: {}", e);
                crate::audio::recording_preferences::RecordingPreferences::default()
            }
        };
        let minutes = prefs.silence_reminder_minutes;
        let mut schedule = (minutes > 0)
            .then(|| ReminderSchedule::new(Duration::from_secs(u64::from(minutes) * 60), REPEAT_EVERY));
        let mut call_tracker =
            (prefs.stop_when_call_ends && cfg!(target_os = "windows")).then(CallEndTracker::default);
        info!(
            "Recording watch started; silence reminder after {}; stop when the call ends: {}",
            if minutes > 0 { format!("{} min", minutes) } else { "never (off)".to_string() },
            call_tracker.is_some()
        );

        let mut was_paused = false;
        let mut last_apps: Option<Vec<String>> = None;
        let mut detection_failed = false;
        loop {
            tokio::time::sleep(CHECK_EVERY).await;
            if WATCH_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            if !crate::audio::recording_commands::is_recording().await {
                // The tray update on stop already did this; repeat it in case a tooltip
                // update from this loop raced with it.
                crate::recording_indicator::apply(&app, crate::recording_indicator::Indicator::Off);
                return;
            }

            let paused = crate::audio::recording_commands::is_recording_paused().await;
            let active_secs = crate::audio::recording_commands::active_recording_seconds().unwrap_or(0.0);
            crate::recording_indicator::show_duration(&app, paused, active_secs as u64);

            if paused {
                was_paused = true;
                continue;
            }
            if was_paused {
                // Paused time does not count as silence.
                was_paused = false;
                mark_speech();
                if let Some(schedule) = schedule.as_mut() {
                    schedule.reset();
                }
                continue;
            }

            if let Some(schedule) = schedule.as_mut() {
                let silent_for = silence();
                if schedule.due(silent_for) {
                    info!("Silence reminder: no speech for {} s", silent_for.as_secs());
                    crate::audio::post_recording::notify(&app, reminder_notification(silent_for)).await;
                }
            }

            if let Some(tracker) = call_tracker.as_mut() {
                let apps = tokio::task::spawn_blocking(other_apps_using_microphone)
                    .await
                    .ok()
                    .flatten();
                let Some(apps) = apps else {
                    if !detection_failed {
                        warn!("Call detection unavailable for now; not stopping when the call ends");
                        detection_failed = true;
                    }
                    continue;
                };
                detection_failed = false;
                if last_apps.as_ref() != Some(&apps) {
                    // Program names only, for diagnosing which app holds the microphone.
                    info!("Call detection: other apps using a microphone: {:?}", apps);
                    last_apps = Some(apps.clone());
                }
                let now = Duration::from_millis(now_ms());
                if let Some(change) = tracker.observe(now, !apps.is_empty()) {
                    info!("Call detection: {:?}", change);
                }
                if tracker.should_stop(now, silence()) {
                    info!(
                        "Stopping the recording: the call ended and there was no speech for {} s",
                        silence().as_secs()
                    );
                    crate::tray::stop_recording_in_background(&app);
                    crate::audio::post_recording::notify(&app, call_ended_notification()).await;
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn reminds_once_the_threshold_is_reached_then_every_repeat() {
        let mut schedule = ReminderSchedule::new(MIN, 10 * MIN);
        assert!(!schedule.due(Duration::from_secs(55)));
        assert!(schedule.due(Duration::from_secs(60)));
        assert!(!schedule.due(Duration::from_secs(65)));
        assert!(!schedule.due(Duration::from_secs(60 + 9 * 60)));
        assert!(schedule.due(Duration::from_secs(60 + 10 * 60)));
        assert!(!schedule.due(Duration::from_secs(60 + 10 * 60 + 5)));
        assert!(schedule.due(Duration::from_secs(60 + 20 * 60 + 3)));
    }

    #[test]
    fn speech_starts_a_new_quiet_stretch() {
        let mut schedule = ReminderSchedule::new(MIN, 10 * MIN);
        assert!(schedule.due(Duration::from_secs(61)));
        // Someone spoke: silence drops back below the threshold.
        assert!(!schedule.due(Duration::from_secs(3)));
        // The next stretch reminds again at the threshold, not after the repeat interval.
        assert!(schedule.due(Duration::from_secs(62)));
    }

    #[test]
    fn reset_reminds_again_at_the_threshold() {
        let mut schedule = ReminderSchedule::new(MIN, 10 * MIN);
        assert!(schedule.due(Duration::from_secs(120)));
        schedule.reset();
        assert!(schedule.due(Duration::from_secs(125)));
    }

    #[test]
    fn describes_silence_in_minutes_and_hours() {
        assert_eq!(describe(Duration::from_secs(61)), "1 minute");
        assert_eq!(describe(Duration::from_secs(12 * 60 + 30)), "12 minutes");
        assert_eq!(describe(Duration::from_secs(120 * 60)), "2 h");
        assert_eq!(describe(Duration::from_secs(125 * 60)), "2 h 5 min");
    }
}
