// audio/post_recording.rs
//
// Post-recording pipeline for sessions recorded with "transcribe after recording".
// Once the frontend has saved the meeting, this transcribes the saved audio with the
// batch path used by Retranscribe, generates the summary, and sends a system
// notification. Jobs run in the background, one meeting at a time.

use crate::audio::retranscription::{is_retranscription_in_progress, start_retranscription};
use crate::audio::speaker_tracks;
use crate::database::repositories::{
    meeting::MeetingsRepository, setting::SettingsRepository, summary::SummaryProcessesRepository,
};
use crate::notifications::commands::NotificationManagerState;
use crate::notifications::types::{Notification, NotificationType};
use crate::state::AppState;
use crate::summary::commands::{begin_summary_process, summary_process_id};
use crate::summary::language_detection::detect_summary_language;
use crate::summary::metadata::{
    read_summary_language_from_metadata, write_detected_summary_language_to_metadata,
};
use crate::summary::service::SummaryService;
use log::{error, info, warn};
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

/// Summary template used for automatic summaries (the meeting page's default).
const SUMMARY_TEMPLATE_ID: &str = "standard_meeting";
/// Same chunking the meeting page sends to `api_process_transcript`.
const SUMMARY_CHUNK_SIZE: i32 = 40000;
const SUMMARY_OVERLAP: i32 = 1000;
/// Event carrying every `PostRecordingStatus` change.
const STATUS_EVENT: &str = "post-recording-status";

/// Serializes jobs: the transcription engine and the retranscription guard are global.
static PIPELINE_QUEUE: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Status of meetings that are queued or being processed. Terminal states are
/// emitted once and then dropped, so a page opened later just shows saved data.
static PIPELINE_STATUS: LazyLock<Mutex<HashMap<String, PostRecordingStatus>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostRecordingStage {
    Queued,
    Transcribing,
    Summarizing,
    Completed,
    Failed,
}

impl PostRecordingStage {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PostRecordingStatus {
    pub meeting_id: String,
    pub stage: PostRecordingStage,
    pub message: String,
    /// Set while summarizing: the summary process id the meeting page can poll.
    pub summary_process_id: Option<String>,
    pub error: Option<String>,
}

fn publish<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    stage: PostRecordingStage,
    message: impl Into<String>,
    summary_process_id: Option<String>,
    error: Option<String>,
) {
    let status = PostRecordingStatus {
        meeting_id: meeting_id.to_string(),
        stage,
        message: message.into(),
        summary_process_id,
        error,
    };
    {
        let mut map = PIPELINE_STATUS.lock().unwrap_or_else(|e| e.into_inner());
        if stage.is_terminal() {
            map.remove(meeting_id);
        } else {
            map.insert(meeting_id.to_string(), status.clone());
        }
    }
    let _ = app.emit(STATUS_EVENT, status);
}

/// Queue the post-recording pipeline for a saved meeting. Returns immediately.
#[tauri::command]
pub async fn start_post_recording_processing<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    folder_path: String,
) -> Result<(), String> {
    if meeting_id.trim().is_empty() || folder_path.trim().is_empty() {
        return Err("A saved meeting and its recording folder are required".to_string());
    }
    if !Path::new(&folder_path).is_dir() {
        return Err("The recording folder for this meeting was not found".to_string());
    }
    {
        let map = PIPELINE_STATUS.lock().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(&meeting_id) {
            return Err("This meeting is already being processed".to_string());
        }
    }

    publish(
        &app,
        &meeting_id,
        PostRecordingStage::Queued,
        "Waiting to transcribe…",
        None,
        None,
    );
    tauri::async_runtime::spawn(run_pipeline(app, meeting_id, folder_path));
    Ok(())
}

/// Current pipeline status for a meeting, if it is queued or in progress.
#[tauri::command]
pub async fn get_post_recording_status(meeting_id: String) -> Option<PostRecordingStatus> {
    PIPELINE_STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&meeting_id)
        .cloned()
}

async fn run_pipeline<R: Runtime>(app: AppHandle<R>, meeting_id: String, folder_path: String) {
    let _turn = PIPELINE_QUEUE.lock().await;
    info!("Post-recording pipeline started for meeting {}", meeting_id);

    let Some(pool) = app
        .try_state::<AppState>()
        .map(|state| state.db_manager.pool().clone())
    else {
        fail(&app, &meeting_id, "Processing failed", "App state is not available").await;
        return;
    };

    // A manual Retranscribe holds the same engine; wait for it rather than failing.
    while is_retranscription_in_progress() {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    publish(
        &app,
        &meeting_id,
        PostRecordingStage::Transcribing,
        "Transcribing recording…",
        None,
        None,
    );

    let provider = match SettingsRepository::get_transcript_config(&pool).await {
        Ok(Some(config)) if !config.provider.is_empty() => config.provider,
        Ok(_) => "parakeet".to_string(),
        Err(e) => {
            warn!("Could not read transcript config, defaulting to parakeet: {}", e);
            "parakeet".to_string()
        }
    };
    let language = crate::get_language_preference_internal();

    // Model `None` uses the configured model, exactly like recording would.
    match start_retranscription(
        app.clone(),
        meeting_id.clone(),
        folder_path.clone(),
        language,
        None,
        Some(provider),
    )
    .await
    {
        Ok(result) if result.segments_count > 0 => {
            info!(
                "Post-recording transcription finished for meeting {}: {} segments",
                meeting_id, result.segments_count
            );
        }
        Ok(_) => {
            fail(&app, &meeting_id, "No speech found", "The recording contains no transcribable speech.").await;
            return;
        }
        Err(e) => {
            fail(&app, &meeting_id, "Transcription failed", &e.to_string()).await;
            return;
        }
    }

    match summarize(&app, &pool, &meeting_id, &folder_path).await {
        Ok(SummaryOutcome::Completed) => {
            let title = meeting_title(&pool, &meeting_id).await;
            publish(&app, &meeting_id, PostRecordingStage::Completed, "Meeting notes ready", None, None);
            notify(
                &app,
                Notification::new("Meeting notes ready", title, NotificationType::TranscriptionComplete),
            )
            .await;
        }
        Ok(SummaryOutcome::Skipped(reason)) => {
            let title = meeting_title(&pool, &meeting_id).await;
            publish(&app, &meeting_id, PostRecordingStage::Completed, reason.clone(), None, None);
            notify(
                &app,
                Notification::new(
                    "Transcript ready",
                    format!("{} — {}", title, reason),
                    NotificationType::TranscriptionComplete,
                ),
            )
            .await;
        }
        Ok(SummaryOutcome::Cancelled) => {
            publish(&app, &meeting_id, PostRecordingStage::Completed, "Summary cancelled", None, None);
        }
        Err(e) => {
            fail(&app, &meeting_id, "Summary failed", &e).await;
        }
    }
}

enum SummaryOutcome {
    Completed,
    Cancelled,
    /// The transcript is saved but no summary was generated, with the reason.
    Skipped(String),
}

async fn summarize<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
    folder_path: &str,
) -> Result<SummaryOutcome, String> {
    let config = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|e| format!("Could not read the summary model settings: {}", e))?;
    let Some(config) = config.filter(|c| !c.provider.is_empty() && !c.model.is_empty()) else {
        return Ok(SummaryOutcome::Skipped(
            "no summary model is configured".to_string(),
        ));
    };
    let provider = config.provider;
    let mut model = config.model;
    // Matches the meeting page: custom endpoints store their model separately.
    if provider == "custom-openai" {
        if let Ok(Some(custom)) = SettingsRepository::get_custom_openai_config(pool).await {
            if !custom.model.is_empty() {
                model = custom.model;
            }
        }
    }

    let segments: Vec<(String, Option<f64>, String, Option<String>)> = sqlx::query_as(
        "SELECT transcript, audio_start_time, timestamp, speaker FROM transcripts
         WHERE meeting_id = ? ORDER BY audio_start_time ASC",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Could not load the transcript: {}", e))?;
    if segments.is_empty() {
        return Ok(SummaryOutcome::Skipped("the transcript is empty".to_string()));
    }

    let text = format_transcript_for_summary(&segments);
    let texts: Vec<String> = segments.iter().map(|(t, _, _, _)| t.clone()).collect();
    let summary_language = resolve_summary_language(Path::new(folder_path), &texts);

    let (started_at, cancellation_token) = begin_summary_process(
        pool,
        meeting_id,
        &text,
        &provider,
        &model,
        SUMMARY_CHUNK_SIZE,
        SUMMARY_OVERLAP,
    )
    .await?;
    publish(
        app,
        meeting_id,
        PostRecordingStage::Summarizing,
        "Generating summary…",
        Some(summary_process_id(started_at)),
        None,
    );
    info!(
        "Post-recording summary started for meeting {} with {}/{}",
        meeting_id, provider, model
    );

    SummaryService::process_transcript_background(
        app.clone(),
        pool.clone(),
        meeting_id.to_string(),
        started_at,
        cancellation_token,
        text,
        provider,
        model,
        String::new(),
        SUMMARY_TEMPLATE_ID.to_string(),
        summary_language,
    )
    .await;

    let process = SummaryProcessesRepository::get_summary_data(pool, meeting_id)
        .await
        .map_err(|e| format!("Could not read the summary result: {}", e))?
        .ok_or_else(|| "The summary result was not saved".to_string())?;
    match process.status.to_lowercase().as_str() {
        "completed" => Ok(SummaryOutcome::Completed),
        "cancelled" => Ok(SummaryOutcome::Cancelled),
        _ => Err(process
            .error
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "Summary generation failed".to_string())),
    }
}

/// "[mm:ss] text" lines, the format the meeting page sends for summaries, with
/// "Me:" / "Others:" before the text when the speaker side is known.
fn format_transcript_for_summary(segments: &[(String, Option<f64>, String, Option<String>)]) -> String {
    segments
        .iter()
        .map(|(text, start, timestamp, speaker)| {
            let time = match start {
                Some(seconds) => {
                    let total = seconds.max(0.0).floor() as u64;
                    format!("[{:02}:{:02}]", total / 60, total % 60)
                }
                None => timestamp.clone(),
            };
            match speaker_tracks::summary_label(speaker.as_deref()) {
                Some(label) => format!("{} {}: {}", time, label, text),
                None => format!("{} {}", time, text),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Per-meeting language if set (the pinned default is written there on save),
/// otherwise detect it from the new transcript and cache it like the meeting page does.
fn resolve_summary_language(folder: &Path, texts: &[String]) -> Option<String> {
    match read_summary_language_from_metadata(folder) {
        Ok(Some(language)) => return Some(language),
        Ok(None) => {}
        Err(e) => warn!("Could not read the meeting summary language: {}", e),
    }
    let detection = detect_summary_language(texts);
    if let Some(language) = detection.language.as_deref() {
        if let Err(e) = write_detected_summary_language_to_metadata(folder, Some(language)) {
            warn!("Could not cache the detected summary language: {}", e);
        }
    }
    detection.language
}

async fn meeting_title(pool: &SqlitePool, meeting_id: &str) -> String {
    match MeetingsRepository::get_meeting_metadata(pool, meeting_id).await {
        Ok(Some(meeting)) if !meeting.title.trim().is_empty() => meeting.title,
        _ => "Your meeting".to_string(),
    }
}

async fn fail<R: Runtime>(app: &AppHandle<R>, meeting_id: &str, title: &str, error_message: &str) {
    error!(
        "Post-recording pipeline failed for meeting {}: {}; error_bytes={}",
        meeting_id,
        title,
        error_message.len()
    );
    publish(
        app,
        meeting_id,
        PostRecordingStage::Failed,
        title,
        None,
        Some(error_message.to_string()),
    );
    notify(
        app,
        Notification::new(
            title,
            format!("{}. Open the meeting to retry.", error_message.trim_end_matches('.')),
            NotificationType::SystemError(error_message.to_string()),
        ),
    )
    .await;
}

/// Shows a notification through the manager (respects the user's notification
/// settings), falling back to a direct system notification before it is ready.
async fn notify<R: Runtime>(app: &AppHandle<R>, notification: Notification) {
    if let Some(state) = app.try_state::<NotificationManagerState<R>>() {
        let guard = state.read().await;
        if let Some(manager) = guard.as_ref() {
            if let Err(e) = manager.show_notification(notification).await {
                warn!("Could not show post-recording notification: {}", e);
            }
            return;
        }
    }
    if let Err(e) = app
        .notification()
        .builder()
        .title(&notification.title)
        .body(&notification.body)
        .show()
    {
        warn!("Could not show post-recording notification: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_transcript_with_minute_second_prefixes() {
        let segments = vec![
            ("Hello".to_string(), Some(5.9), "10:00".to_string(), None),
            ("Later".to_string(), Some(125.0), "10:02".to_string(), None),
            ("No offset".to_string(), None, "[10:03]".to_string(), None),
        ];
        assert_eq!(
            format_transcript_for_summary(&segments),
            "[00:05] Hello\n[02:05] Later\n[10:03] No offset"
        );
    }

    #[test]
    fn labels_speaker_sides_for_the_summary() {
        let segments = vec![
            ("And one is not responding.".to_string(), Some(12.0), "10:00".to_string(), Some("mic".to_string())),
            ("I had to connect my headphones.".to_string(), Some(14.0), "10:00".to_string(), Some("system".to_string())),
        ];
        assert_eq!(
            format_transcript_for_summary(&segments),
            "[00:12] Me: And one is not responding.\n[00:14] Others: I had to connect my headphones."
        );
    }

    #[test]
    fn only_terminal_stages_are_dropped_from_status() {
        assert!(PostRecordingStage::Completed.is_terminal());
        assert!(PostRecordingStage::Failed.is_terminal());
        assert!(!PostRecordingStage::Queued.is_terminal());
        assert!(!PostRecordingStage::Transcribing.is_terminal());
        assert!(!PostRecordingStage::Summarizing.is_terminal());
    }
}
