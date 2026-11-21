import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type PostRecordingStage = 'queued' | 'transcribing' | 'summarizing' | 'completed' | 'failed';

/** Mirrors `PostRecordingStatus` in src-tauri/src/audio/post_recording.rs */
export interface PostRecordingStatus {
  meeting_id: string;
  stage: PostRecordingStage;
  message: string;
  summary_process_id: string | null;
  error: string | null;
}

export interface PostRecordingProgress {
  percentage: number;
  message: string;
}

interface RetranscriptionProgressPayload {
  meeting_id: string;
  stage: string;
  progress_percentage: number;
  message: string;
}

interface PostRecordingHandlers {
  /** The new transcript has been saved to the database. */
  onTranscriptReady?: () => void;
  /** A summary process exists for the meeting and can be polled. */
  onSummaryStarted?: () => void;
  /** The pipeline finished; `previousStage` is the last stage before it ended. */
  onFinished?: (status: PostRecordingStatus, previousStage: PostRecordingStage | null) => void;
}

/**
 * Tracks the post-recording pipeline (transcribe saved audio → summary → notify)
 * for one meeting. Returns null status when nothing is queued or running.
 */
export function usePostRecordingStatus(meetingId: string | null, handlers: PostRecordingHandlers) {
  const [status, setStatus] = useState<PostRecordingStatus | null>(null);
  const [progress, setProgress] = useState<PostRecordingProgress | null>(null);
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  const lastStageRef = useRef<PostRecordingStage | null>(null);

  useEffect(() => {
    setStatus(null);
    setProgress(null);
    lastStageRef.current = null;
    if (!meetingId) return;

    let cancelled = false;
    let unlisteners: Array<() => void> = [];

    const subscribe = async () => {
      const subscriptions = await Promise.all([
        listen<PostRecordingStatus>('post-recording-status', ({ payload }) => {
          if (payload.meeting_id !== meetingId) return;
          const previousStage = lastStageRef.current;
          lastStageRef.current = payload.stage;
          if (payload.stage === 'completed' || payload.stage === 'failed') {
            setStatus(null);
            setProgress(null);
            handlersRef.current.onFinished?.(payload, previousStage);
            return;
          }
          setStatus(payload);
          if (payload.stage === 'summarizing') {
            setProgress(null);
            handlersRef.current.onSummaryStarted?.();
          }
        }),
        listen<RetranscriptionProgressPayload>('retranscription-progress', ({ payload }) => {
          if (payload.meeting_id !== meetingId) return;
          setProgress({ percentage: payload.progress_percentage, message: payload.message });
        }),
        listen<{ meeting_id: string }>('retranscription-complete', ({ payload }) => {
          if (payload.meeting_id !== meetingId) return;
          handlersRef.current.onTranscriptReady?.();
        }),
      ]);

      if (cancelled) {
        subscriptions.forEach((unlisten) => unlisten());
        return;
      }
      unlisteners = subscriptions;

      // Pick up a job that started before this page mounted.
      try {
        const current = await invoke<PostRecordingStatus | null>('get_post_recording_status', { meetingId });
        if (!cancelled && current) {
          lastStageRef.current = lastStageRef.current ?? current.stage;
          setStatus((previous) => previous ?? current);
        }
      } catch (error) {
        console.warn('Failed to read post-recording status:', error);
      }
    };

    subscribe().catch((error) => console.error('Failed to subscribe to post-recording events:', error));

    return () => {
      cancelled = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [meetingId]);

  return { status, progress };
}
