'use client';

import { Loader2 } from 'lucide-react';
import type {
  PostRecordingProgress,
  PostRecordingStatus,
} from '@/hooks/meeting-details/usePostRecordingStatus';

interface PostRecordingBannerProps {
  status: PostRecordingStatus;
  progress: PostRecordingProgress | null;
}

/** Slim status bar shown on the meeting page while the post-recording pipeline runs. */
export function PostRecordingBanner({ status, progress }: PostRecordingBannerProps) {
  const transcribing = status.stage === 'transcribing';
  const label = transcribing && progress ? progress.message : status.message;
  const percentage = transcribing && progress ? Math.min(100, Math.max(0, progress.percentage)) : null;

  return (
    <div
      role="status"
      aria-live="polite"
      className="flex items-center gap-3 border-b border-blue-100 bg-blue-50 px-4 py-2 text-sm text-blue-900"
    >
      <Loader2 className="h-4 w-4 shrink-0 animate-spin" aria-hidden="true" />
      <span className="min-w-0 truncate">{label}</span>
      {percentage !== null ? (
        <div className="ml-auto flex w-40 shrink-0 items-center gap-2">
          <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-blue-100">
            <div
              className="h-full rounded-full bg-blue-500 transition-all duration-500"
              style={{ width: `${percentage}%` }}
            />
          </div>
          <span className="w-9 text-right text-xs tabular-nums">{percentage}%</span>
        </div>
      ) : (
        <span className="ml-auto hidden shrink-0 text-xs text-blue-700 sm:inline">
          You&apos;ll get a notification when the notes are ready
        </span>
      )}
    </div>
  );
}
