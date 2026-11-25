/**
 * Speaker side of a transcript line, stored by retranscription when the recording kept
 * separate tracks: "mic" is the person recording, "system" is everyone in the call.
 * Mirrors `SpeakerSide` in src-tauri/src/audio/speaker_tracks.rs.
 */
export type SpeakerSide = 'mic' | 'system';

/** Label shown next to a line in the transcript view. */
export function speakerDisplayLabel(speaker?: string | null): string | null {
  if (speaker === 'mic') return 'You';
  if (speaker === 'system') return 'Others';
  return null;
}

/**
 * Prefix for transcript text that is copied or sent to the summary model. Must match
 * `summary_label` in speaker_tracks.rs so both summary paths send identical text.
 */
export function speakerTextPrefix(speaker?: string | null): string {
  if (speaker === 'mic') return 'Me: ';
  if (speaker === 'system') return 'Others: ';
  return '';
}
