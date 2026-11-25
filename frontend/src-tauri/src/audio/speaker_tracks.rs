//! Separate microphone and system-audio tracks of a recording, used to label the
//! transcript by speaker side: the microphone is the person recording ("Me"), system audio
//! is everyone else in the call ("Others"). The mixed `audio.mp4` stays the playback file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Folder inside the meeting folder that holds `mic.mp4` and `system.mp4`.
pub const TRACKS_DIR: &str = "tracks";
/// Kept apart from `.checkpoints/`, which crash recovery merges into `audio.mp4`.
const TRACK_CHECKPOINTS_DIR: &str = ".track-checkpoints";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerSide {
    Mic,
    System,
}

impl SpeakerSide {
    /// Value stored in `transcripts.speaker` (migration 20251110000001).
    pub fn db_value(self) -> &'static str {
        match self {
            SpeakerSide::Mic => "mic",
            SpeakerSide::System => "system",
        }
    }
}

pub fn track_path(meeting_folder: &Path, side: SpeakerSide) -> PathBuf {
    meeting_folder
        .join(TRACKS_DIR)
        .join(format!("{}.mp4", side.db_value()))
}

pub fn track_checkpoints_root(meeting_folder: &Path) -> PathBuf {
    meeting_folder.join(TRACK_CHECKPOINTS_DIR)
}

pub fn track_checkpoints_dir(meeting_folder: &Path, side: SpeakerSide) -> PathBuf {
    track_checkpoints_root(meeting_folder).join(side.db_value())
}

/// Both track files (mic, system), if this recording has them.
pub fn find_tracks(meeting_folder: &Path) -> Option<(PathBuf, PathBuf)> {
    let mic = track_path(meeting_folder, SpeakerSide::Mic);
    let system = track_path(meeting_folder, SpeakerSide::System);
    (mic.is_file() && system.is_file()).then_some((mic, system))
}

/// Speaker name used in transcript text sent to the summary model.
pub fn summary_label(speaker: Option<&str>) -> Option<&'static str> {
    match speaker {
        Some("mic") => Some("Me"),
        Some("system") => Some("Others"),
        _ => None,
    }
}

/// A transcribed piece of one track.
#[derive(Debug, Clone, PartialEq)]
pub struct LabeledTranscript {
    pub text: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub speaker: Option<SpeakerSide>,
}

/// How far apart (ms) a microphone segment and a system segment may be and still count as
/// the same moment. VAD boundaries differ per track, and speakers add a little delay.
const ECHO_TIME_SLACK_MS: f64 = 1000.0;
/// Share of a microphone segment's word pairs that must also occur in the system audio
/// at the same moment before it is treated as echo.
const ECHO_MIN_SHARED_PAIRS: f64 = 0.5;

/// Removes microphone segments that are the call audio picked up by the microphone
/// (laptop speakers instead of a headset). Such a segment repeats, at the same moment,
/// what the system track already contains; keeping it would show the other person's
/// words twice, once as "Me".
pub fn drop_echoed_mic_segments(segments: Vec<LabeledTranscript>) -> Vec<LabeledTranscript> {
    let system: Vec<&LabeledTranscript> = segments
        .iter()
        .filter(|s| s.speaker == Some(SpeakerSide::System))
        .collect();

    let echoed: Vec<bool> = segments
        .iter()
        .map(|s| s.speaker == Some(SpeakerSide::Mic) && is_echo(s, &system))
        .collect();

    segments
        .into_iter()
        .zip(echoed)
        .filter_map(|(segment, echo)| (!echo).then_some(segment))
        .collect()
}

fn is_echo(mic: &LabeledTranscript, system: &[&LabeledTranscript]) -> bool {
    let nearby: Vec<&&LabeledTranscript> = system
        .iter()
        .filter(|s| {
            s.start_ms < mic.end_ms + ECHO_TIME_SLACK_MS && s.end_ms > mic.start_ms - ECHO_TIME_SLACK_MS
        })
        .collect();
    if nearby.is_empty() {
        return false;
    }

    let mic_words = words(&mic.text);
    if mic_words.is_empty() {
        return false;
    }
    let system_words: Vec<String> = nearby.iter().flat_map(|s| words(&s.text)).collect();

    if mic_words.len() >= 3 {
        let system_pairs: HashSet<(&str, &str)> = pairs(&system_words).collect();
        let mic_pairs: Vec<(&str, &str)> = pairs(&mic_words).collect();
        let shared = mic_pairs.iter().filter(|p| system_pairs.contains(p)).count();
        return shared as f64 / mic_pairs.len() as f64 >= ECHO_MIN_SHARED_PAIRS;
    }

    // One or two words ("okay", "yes please"): too short to compare word pairs, so also
    // require the system audio to be speaking for most of the segment.
    let system_set: HashSet<&str> = system_words.iter().map(String::as_str).collect();
    let all_words_heard = mic_words.iter().all(|w| system_set.contains(w.as_str()));
    let mic_duration = (mic.end_ms - mic.start_ms).max(1.0);
    let overlap: f64 = nearby
        .iter()
        .map(|s| (s.end_ms.min(mic.end_ms) - s.start_ms.max(mic.start_ms)).max(0.0))
        .sum();
    all_words_heard && overlap / mic_duration >= 0.5
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn pairs(words: &[String]) -> impl Iterator<Item = (&str, &str)> {
    words.windows(2).map(|w| (w[0].as_str(), w[1].as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, start_ms: f64, end_ms: f64, speaker: SpeakerSide) -> LabeledTranscript {
        LabeledTranscript { text: text.to_string(), start_ms, end_ms, speaker: Some(speaker) }
    }

    #[test]
    fn track_paths_stay_out_of_crash_recovery_checkpoints() {
        let folder = Path::new("meeting");
        assert_eq!(track_path(folder, SpeakerSide::Mic), folder.join("tracks").join("mic.mp4"));
        assert_eq!(track_path(folder, SpeakerSide::System), folder.join("tracks").join("system.mp4"));
        assert!(!track_checkpoints_dir(folder, SpeakerSide::Mic).starts_with(folder.join(".checkpoints")));
    }

    #[test]
    fn summary_labels_match_stored_speaker_values() {
        assert_eq!(summary_label(Some(SpeakerSide::Mic.db_value())), Some("Me"));
        assert_eq!(summary_label(Some(SpeakerSide::System.db_value())), Some("Others"));
        assert_eq!(summary_label(None), None);
    }

    #[test]
    fn mic_echo_of_the_call_is_dropped() {
        let segments = vec![
            seg("I had to connect my headphones, sorry about that", 10_000.0, 14_000.0, SpeakerSide::System),
            seg("had to connect my head phones sorry about", 10_200.0, 14_300.0, SpeakerSide::Mic),
        ];
        let kept = drop_echoed_mic_segments(segments);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].speaker, Some(SpeakerSide::System));
    }

    #[test]
    fn own_speech_next_to_the_call_is_kept() {
        let segments = vec![
            seg("And one is not responding.", 5_000.0, 7_000.0, SpeakerSide::Mic),
            seg("I had to connect my headphones.", 7_200.0, 9_000.0, SpeakerSide::System),
            seg("I think that is the plan, so yes", 20_000.0, 23_000.0, SpeakerSide::System),
            seg("Yes, I think so too", 21_000.0, 22_500.0, SpeakerSide::Mic),
        ];
        let kept = drop_echoed_mic_segments(segments.clone());
        assert_eq!(kept, segments);
    }

    #[test]
    fn short_mic_echo_needs_the_call_to_be_speaking() {
        let echo = vec![
            seg("Okay.", 3_000.0, 3_600.0, SpeakerSide::System),
            seg("okay", 3_050.0, 3_650.0, SpeakerSide::Mic),
        ];
        assert_eq!(drop_echoed_mic_segments(echo).len(), 1);

        // Same word, but the call was quiet for most of it: a separate reply.
        let reply = vec![
            seg("Okay.", 3_000.0, 3_300.0, SpeakerSide::System),
            seg("okay", 3_600.0, 4_400.0, SpeakerSide::Mic),
        ];
        assert_eq!(drop_echoed_mic_segments(reply).len(), 2);
    }

    #[test]
    fn segments_without_a_speaker_are_untouched() {
        let segments = vec![LabeledTranscript {
            text: "hello there everyone".into(),
            start_ms: 0.0,
            end_ms: 1_000.0,
            speaker: None,
        }];
        assert_eq!(drop_echoed_mic_segments(segments.clone()), segments);
    }
}
