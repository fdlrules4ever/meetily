# MyMeetily fork

Fork of [Zackriya-Solutions/meetily](https://github.com/Zackriya-Solutions/meetily) with one behaviour change:
**transcribe after recording** instead of live.

## What is different from upstream

- **Recording Settings → "Transcribe After Recording"** (on by default). Records audio only. When the
  recording stops, the saved file is transcribed with the batch path used by Retranscribe, the summary
  is generated (template `standard_meeting`, configured summary model) and a Windows notification is
  shown. Turn it off for upstream's live transcription. Audio is always saved in this mode.
  - Pipeline: `frontend/src-tauri/src/audio/post_recording.rs` (commands `start_post_recording_processing`,
    `get_post_recording_status`, event `post-recording-status`).
  - Started from `frontend/src/hooks/useRecordingStop.ts` after the meeting is saved; progress banner on
    the meeting page (`usePostRecordingStatus.ts`, `PostRecordingBanner.tsx`).
  - Starting a recording in this mode only checks that the configured speech model is downloaded
    (`validate_transcription_model_downloaded`); it is loaded once, after the recording.
- **Settings → Prompts:** edit summary templates (structured form) and the base instructions of the final
  summary prompt (rules 2-8; rule 1 "write in English" stays fixed for the translation step). Overrides are
  stored in `%APPDATA%\Meetily\templates\` and `%APPDATA%\Meetily\prompts\summary_instructions.md`
  (`summary/templates/loader.rs`, `summary/instructions.rs`, `components/SummaryPromptSettings.tsx`).
- **Updates:** no automatic update check at startup and no updater artifacts. Upstream updates would
  replace this build. Check manually from About if needed.
- **"You" / "Others" speaker labels.** In record-only mode the unmixed microphone and system audio are
  also saved as `tracks/mic.mp4` and `tracks/system.mp4` (64 kbit/s, next to the mixed `audio.mp4`, which
  is unchanged and still used for playback). Retranscription transcribes the two tracks separately and
  stores `mic` / `system` in the existing `transcripts.speaker` column. Microphone lines that repeat the
  call audio at the same moment (laptop speakers instead of a headset) are dropped. Without any call
  audio (in-person meeting) lines stay unlabelled. The summary model receives "Me:" / "Others:" prefixes.
  Older recordings and imports have no tracks and are transcribed from the mixed file as before.
  - Recording: `pipeline.rs` (`track_sender`), `recording_saver.rs` (`start_track_accumulation`),
    `incremental_saver.rs` (`new_track`). Transcription: `retranscription.rs`, `speaker_tracks.rs`.
    UI: `VirtualizedTranscriptView.tsx`, `lib/speaker.ts`.
- **"Still recording?" reminder.** Recording Settings → "Still Recording Reminder" (default: after 1 minute,
  options off to 30 minutes). The pipeline's VAD on the mixed microphone + call audio marks every moment it
  hears speech; a watcher started with each recording shows a notification once nobody has spoken for that long,
  then every 10 minutes while it stays quiet. Speech (not loudness) resets it; paused time doesn't count. The
  setting is read when a recording starts. `audio/recording_watch.rs`, hook in `pipeline.rs` (`mark_speech`).
- **Stop when the call ends (Windows).** Recording Settings → "Stop When the Call Ends" (on by default). Call
  apps (Teams, Zoom, a browser with Meet) hold the microphone for the whole call, also while muted, and release
  it when the call ends. Every 5 s the recording watch lists the other programs with an active capture session
  on any microphone (WASAPI audio sessions, Meetily itself excluded). After a call of at least 2 minutes is over,
  the recording stops once nobody has spoken for a minute, at the latest 10 minutes after the call; rejoining
  cancels it. It stops like the tray's Stop (saved, transcribed, summarized) and shows a notification. Logs list
  the program names holding the microphone. Apps that keep the microphone open all the time (e.g. noise
  suppression tools) prevent the stop. `audio/call_detection.rs`, `audio/recording_watch.rs`.
- **Recording indicator.** While recording: red dot on the tray icon (orange when paused) with the recording
  time in its tooltip, red dot on the taskbar button (Windows overlay icon) and a Dock badge (macOS, untested).
  The tray icon matters most, because closing the window hides it to the tray. `recording_indicator.rs`, applied
  from `tray::update_tray_menu_async`.
- **Whisper thread count:** upstream computes a thread count per machine but never passes it to
  whisper.cpp, which then uses its default of 4. `whisper_engine.rs` now calls `set_n_threads`.
- **Welcome screen:** says the transcript is made after recording, and why (only while "Transcribe After
  Recording" is on).

## Building on Windows (verified 2026-11-21)

Prerequisites: VS 2022 Build Tools with the C++ workload, CMake, rustup, Node 22.

1. **Rust 1.99.1** (upstream CI pin). Rust 1.100 changed the `target/` build-dir layout, which breaks
   `ort-sys 2.0.0-rc.10`'s DLL copy step.
   ```powershell
   rustup toolchain install 1.99.1 --profile minimal
   rustup override set 1.99.1
   ```
2. **libclang 20** for `whisper-rs-sys` bindgen. LLVM 23 makes it generate empty (opaque) structs.
   Point `LIBCLANG_PATH` at an LLVM 20 `bin` folder that has `libclang.dll` and `..\lib\clang\20\include`
   (locally: `.toolchain\llvm20\bin`, taken from the official `clang+llvm-20.1.8-x86_64-pc-windows-msvc.tar.xz`).
3. **ONNX Runtime 1.22.0.** The GitHub URL in `frontend/src-tauri/build/onnxruntime.rs` returns 404
   (the `microsoft/onnxruntime` repo was unreachable on 2026-11-21). Take `onnxruntime.dll`,
   `onnxruntime_providers_shared.dll` and `LICENSE` (as `onnxruntime-LICENSE.txt`) from the NuGet package
   `Microsoft.ML.OnnxRuntime` 1.22.0 (`runtimes/win-x64/native/`) into
   `frontend/src-tauri/binaries/onnxruntime/`. Their SHA-256 match the pins in `onnxruntime.rs`, which the
   build script re-verifies.
4. **llama-helper sidecar:**
   ```powershell
   cargo build -p llama-helper --release
   Copy-Item target\release\llama-helper.exe frontend\src-tauri\binaries\llama-helper-x86_64-pc-windows-msvc.exe
   ```
5. **pnpm 9.15.9** (repo pin). pnpm 10+ ignores `pnpm.overrides` and tries to reinstall `node_modules`
   when running scripts. Locally `.toolchain\bin\pnpm(.cmd)` forwards to `npx -y pnpm@9.15.9`; put it
   first on `PATH` so Tauri's `beforeBuildCommand` uses it too.
   ```powershell
   cd frontend
   pnpm install --frozen-lockfile --ignore-scripts
   pnpm run tauri:build
   ```
   Output: `target\release\bundle\msi\*.msi` and `target\release\bundle\nsis\*-setup.exe`.
6. **GPU (Vulkan), same as upstream's Windows release.** Without it Whisper runs on the CPU only, which
   is far too slow for large models. `scripts/auto-detect-gpu.js` picks CPU when it sees an NVIDIA GPU
   without the CUDA toolkit, so force the feature. Vulkan SDK 1.4.309.0 (upstream's CI pin) installs
   without admin rights and without system changes:
   ```powershell
   .\VulkanSDK-1.4.309.0-Installer.exe --root "$PWD\.toolchain\vulkan-sdk\1.4.309.0" --accept-licenses --default-answer --confirm-command install copy_only=1
   ```
   Then, before step 5's build:
   ```powershell
   $env:VULKAN_SDK = "$PWD\.toolchain\vulkan-sdk\1.4.309.0"
   $env:PATH = "$env:VULKAN_SDK\Bin;$env:PATH"
   $env:TAURI_GPU_FEATURE = "vulkan"
   ```
   The app uses the system's `vulkan-1.dll` (installed with any current GPU driver).

Notes: Installers are
unsigned (SmartScreen warns). The app identifier is unchanged (`com.meetily.ai`), so it installs over
official Meetily and keeps its meetings, models and settings.
