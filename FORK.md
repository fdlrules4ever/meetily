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
