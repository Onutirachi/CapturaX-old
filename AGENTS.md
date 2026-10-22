# CapturaX — Agent Instructions

Tauri 2 desktop app (Windows only) for capture cards: lowest-latency preview to play consoles,
recording, screenshots and streaming by link. **Uses Preact, not React.**

## Build & Dev Commands

```bash
npm run tauri dev      # Dev mode (builds the viewer page, then Tauri + Vite) — or double-click dev.cmd
npm run build          # tsc && vite build && build:viewer
npm run build:viewer   # viewer page only → src-tauri/viewer-dist (embedded in the exe)
npx tauri build        # release exe + NSIS installer (per-machine, creates the firewall rule)
```

Dev server runs on `localhost:1420` (fixed port — fails if unavailable).
Keep the npm `@tauri-apps/*` packages on the same major.minor as the Rust crates, or `tauri build` refuses to run.

## Stack

| Layer | Tech |
|-------|------|
| Frontend | **Preact** 10 (not React), TypeScript 5.6 strict mode |
| Styling | **Tailwind CSS v4** (`@import "tailwindcss"` + `@theme` blocks in CSS) |
| Build | Vite 6 + `@preact/preset-vite` (two configs: app and viewer) |
| Desktop | **Tauri 2** (Rust backend: WASAPI, Media Foundation, axum/hyper, igd-next) |
| JSX | `jsxImportSource: "preact"` — import from `preact/hooks`, not `react` |

## Critical: Preact vs React

- Import hooks from `preact/hooks`: `import { useState, useEffect, useRef } from "preact/hooks"`
- Import component types from `preact`: `import { type ComponentChildren } from "preact"`
- Never `import React from "react"` or use React-specific APIs

## Architecture (hybrid)

The WebView keeps the UI, the video preview (`getUserMedia` + `<video>`) and **video encoding**
(WebCodecs on the GPU). Rust does everything else: audio, muxing, files, network.
Capture cards usually allow a single owner, which is why video capture stays in the WebView.

```
src/main.tsx        → FullscreenProvider > WindowBar + App
src/App.tsx         → VideoStreamProvider > AlertsProvider > NativeProvider > FrameStatsProvider > VideoStream + PermissionGate(Overlay)
src/context/        → VideoStream, FrameStats, Alerts, Fullscreen, Native
src/components/     → Overlay/ (Toolbar, Dock, StatusBar), Settings/ (modal + tabs), VideoStream/, WindowBar/, PermissionGate/
src/core/native/    → api.ts (typed invoke wrappers), clock.ts (performance.now ↔ QPC calibration)
src/core/media/     → presets.ts (quality → VideoEncoderConfig), encoder.worker.ts, EncoderSession.ts
src/core/utils/     → Stream (sources), Resolution, constants, alerts helpers
viewer/             → page for people watching the link (separate Vite entry, reuses src/ components)
src-tauri/src/
  audio/            → WASAPI engine: devices, shared sources, low-latency preview, mixer, per-app capture
  media/            → AAC (Media Foundation), fMP4 muxer, sessions (recording/stream), file sink
  stream/           → HLS hub, hardened HTTP server, UPnP, firewall rule
  commands.rs       → all invoke commands + AppState
  config.rs         → settings.json (app config dir)
```

### Data flow
- **Preview audio**: Rust plays the chosen WASAPI input on the chosen output (~10–20 ms). The `<video>`
  is muted. `audio.nativePreview = false` falls back to the WebView audio track.
- **Recording/stream**: `EncoderSession` clones the video track → `MediaStreamTrackProcessor` → worker
  (`VideoEncoder`, `prefer-hardware`) → packets sent **in order** via `media_video_chunk` (raw binary
  body, metadata in headers) with timestamps converted to the QPC clock. The Rust mixer adds audio
  (device + mic + apps) stamped with the same clock; `media/` muxes into fMP4.
- **Mic on separate tracks** (`audio.recordMicSeparate`, recordings only): the mixer also emits
  "everything but the mic" and "mic only" (`PcmBlock::stems`); the file gets 3 AAC tracks (Mix,
  Game and apps, Microphone) in one alternate group, only the Mix enabled, so players play the mix.
- **Recording file**: fMP4 while running (crash-safe); on stop a full `moov` is appended and old
  `moov`/`moof` boxes are renamed to `free`, so it becomes a regular MP4.
- **Stream (low latency)**: the stream session cuts fMP4 parts every 100 ms (`part_secs`);
  `stream/hub.rs` keeps the parts since the last keyframe and broadcasts new ones. Viewers read
  `/live/<session>/live.mp4` (never-ending response) into MediaSource (`viewer/player.ts`, ~0.4 s
  behind the live edge). Parts are also joined keyframe-to-keyframe into HLS segments for
  Safari/iPhone. Served by `stream/mod.rs` (GET-only fixed routes, no disk access from URLs,
  connection/header limits).
- **Chromium encoder quirks** (`presets.ts`, `media/bitstream.rs`): HEVC needs the
  `PlatformHEVCEncoderSupport` feature (set in `tauri.conf.json` `additionalBrowserArgs`) and comes
  as Annex B without a description; above 1080p, >30 fps is only accepted without `framerate`.
  Capture cards in MJPEG deliver full-range BT.601 frames; the GPU encoder keeps them as-is but
  writes no color info, so players would assume limited range (dark, crushed blacks). The worker
  sends the color the encoder reports (`decoderConfig.colorSpace`) with the config, and Rust
  writes it into the H.264 SPS VUI (`bitstream::set_avc_color`) and a `colr` box (both codecs).

### Outputs (record / stream)
The top bar only picks the preview device: that is what you see and hear. Recording and streaming
share one **Record / Stream** window (`OutputDialog.tsx`, opened by both Dock buttons), in columns:
source + audio (shared), then Recording and/or Streaming, shown only when their switch in the
footer is on (saved in localStorage), next to Close / Start. The **source** (`config.outputSource`) is any device
(`device:<id>`), screen or window; `null` means the preview device. Capture cards allow a single
owner, so whoever opens a device registers its track in `core/native/deviceShare.ts` and the other
side (preview or output) uses a `clone()` instead of opening it again. The audio mix (device audio,
microphone, PC sound, volume under each; "Choose apps" opens a small picker over the window) is the
same for both; its "Auto" device is the audio of the source device (`audio_set_output_paired_input`;
for a screen/window, the preview device's). The footer button does what the switches show
(Start / Stop … / Apply). Source and audio devices are locked only while recording; while only
streaming they can change and "Apply" restarts the stream (`streamDirty` compares against what the
stream started with). With a source other than the preview device, the top bar shows "View output". F10 records
with the last choices. Settings → Audio keeps only what rarely changes.

### P2P link (no port forwarding) — `src/core/p2p/`, `relay/`
The Streaming column of the Record / Stream window has two switches: **P2P** and **Direct link**
(the HTTP link served by this PC). With Direct link off the Rust stream session and its encoder
aren't started at all. P2P needs a signaling server configured in Settings → Streaming
(`stream.p2pServer` + `stream.p2pPassword`); the app ships without one (open source: each owner
runs their own, see `relay/README.md`), and without it the P2P switch is locked with a tooltip.
While streaming, `P2PHost` connects to `wss://<server>/signal` as host (with the password) and opens
one `RTCPeerConnection` per viewer: the output video track plus an audio track generated from the
Rust mix (`p2p_audio_start` posts 10 ms PCM blocks through the WebView2 bridge → `P2PAudio` →
`MediaStreamTrackGenerator`). Media goes straight to each viewer (STUN only, no TURN), so upload is
split per viewer (`stream.p2pMaxViewers`). The room is `sha256(stream.p2pKey)`.
Server passwords (`relay/src/passwords.rs`, salted hashes in the data volume): **admin** (the owner;
streams and changes the usage password through `/api/usage-password`, shown in the app when the
saved password is the admin one) and **usage** (given to friends; changing it disconnects whoever
used the old one). Viewers need no password. `capturax-signal reset-admin` prints a new admin
password. The relay image (`relay/Dockerfile`, context = repo root) also serves the viewer page
(`/v/<room>` → `viewer/p2pPlayer.ts`). Deploy: `relay/deploy/` (shared Caddy proxy for HTTPS +
the CapturaX compose); on the owner's VPS it lives in `/opt/proxy` and `/opt/capturax`.

### Screen/window capture (`src-tauri/src/capture/`)
Screens are named "Screen 2 - 27G2G4" from `\\.\DISPLAYn` + the DisplayConfig monitor name. Windows Graphics
Capture runs in Rust (no yellow border, cursor on) and each BGRA frame is copied into WebView2
shared buffers (`bridge.rs`: `CreateSharedBuffer` + `PostSharedBufferToScript`, a 64-byte header
whose first u32 is 1 while the frame waits for the WebView, and a small `PostWebMessageAsJson` per
frame). `src/core/native/screenCapture.ts` turns them into a `MediaStreamTrackGenerator` track, so
preview, recording, streaming and screenshots work exactly like a capture card. Generated tracks
don't report size/FPS in `getSettings()`: use `trackSettings(track)`. All WebView2 calls happen on
the main thread (`run_on_main_thread`).

### App audio (system audio minus excluded apps)
Process loopback can only include/exclude one process **tree**, so the mixer captures each app with an
audio session separately and sums them, never capturing CapturaX's own tree or a tree that contains an
excluded executable (`audio/apps.rs::plan_captures`). Exclusions are saved by lowercase exe name.

### Languages (`src/i18n/`)
All UI text goes through `t("key", { vars })` (components call `const t = useT()` so they re-render
when the language changes; non-component code imports `t`). `en.ts` defines the keys; other
languages are typed `Messages`, so a missing translation is a compile error. Plurals use
`<key>_one` / `<key>_other` with `{ count }`; `tx()` accepts elements as variables (bold, code).
The language comes from Settings → General (`config.language`) or Windows (`system_languages`);
the viewer page uses the watcher's browser languages. Errors shown to the user are never English
sentences built in Rust: they are `crate::error::coded(code, data)` JSON and `errorText()`
translates `error.<code>`. Dropdown titles are translated, so uncontrolled dropdowns need an
explicit `storageKey`.

### Context Providers
- `VideoStreamContext` — `videoStream`, `videoTrack`, `volume`/`muted`, `ambientLight`
- `FrameStatsContext` — measured `latency` (ms) + `fps`, updated 2×/s; separate so only the StatusBar re-renders
- `AlertsContext` — `alert` `{ text, color, icon, timeout? }`; build them with `alerts.error/warning/info/success`
- `FullscreenContext` — `fullscreen` boolean
- `NativeContext` — Rust config (`updateConfig` is optimistic + debounced save), audio devices,
  media/server status polling, `toggleRecording`/`toggleStreaming`/`screenshot`, settings modal tab.
  Hotkeys: F9 screenshot, F10 record.

### Key Class: `Stream` (`src/core/utils/Stream.ts`)
Video source manager, instantiated once in `ToolbarContent`:
- Real device modes (resolution × FPS × format) come from Media Foundation (`video_modes` command,
  `src-tauri/src/video_modes.rs`), matched to the WebView device by USB VID:PID or name
  (`videoModes.ts`). The toolbar then shows a single **Mode** dropdown with only real modes;
  screen capture / unknown devices keep separate Resolution + Framerate dropdowns.
- Automatic = largest resolution that still does ≥ ~60 fps, at its highest FPS (`chooseMode`).
  This is the capture card's USB output; the console's HDMI output is negotiated by EDID and the
  app cannot see or change it.
- `setVideoDevice(id, modes)`, `setMode()`, `setResolution()`, `setFrameRate()` (`null` = auto)
  **reopen** the device with `resizeMode: "none"` (native mode), coalesced by `requestVideoRestart()`
- `SCREEN_SOURCE` + `getDisplayMedia` is only a fallback outside the app (no `chrome.webview`); in the app,
  screens and windows are output sources, not preview sources
- Browser audio fallback: `updateAudioTrack()` / `clearAudio()`

### Core Reusable Components (`src/core/components/`)
- `DropDown` / `DropOption` — uncontrolled with localStorage persistence (`storageKey`, default = title),
  or controlled with `value` (no persistence, `onChoose` only on user change)
- `ButtonToggle`, `Tag`
- Settings controls live in `src/components/Settings/controls.tsx` (Toggle, Select, Slider, NumberInput, Segmented…)

### Styling Conventions
- Tailwind CSS v4 utility classes directly on JSX elements; tokens in [`src/Globals.css`](src/Globals.css) `@theme`
- Colors: `--color-accent` (#ff7304, logo orange: active states, sliders, main buttons),
  `--color-info` (#3b82f6 blue: informational), `--color-bad` (#ef4444: errors and REC)
- The viewer imports `viewer/viewer.css`, which adds `@source "../src"` so Tailwind sees shared components
- Icons are Tabler outline SVGs in `public/assets/`, rendered as CSS masks by `Overlay/Icon`
- **Asset URLs in CSS must be absolute.** In the built app the CSS lives in `/assets/index.css`, so a
  relative `url(./assets/x.svg)` in a Tailwind class or inside a CSS variable (`--src: url(...)`)
  resolves to `/assets/assets/x.svg` and the icon disappears (dev mode inlines the CSS and hides
  this). Use `/assets/...` in classes and `new URL(src, document.baseURI).href` for variables.

### Tauri-Specific Patterns
- Frameless window: drag region via `data-tauri-drag-region` on `WindowBar`
- Async commands run off the main thread; COM is initialized per thread with `audio::ComGuard` (MTA)
- `windows` crate gotcha: `PROPVARIANT` has a `Drop` that calls `PropVariantClear` — wrap
  hand-built ones (pointing at stack data) in `ManuallyDrop`

## Tests (Rust, ignored by default — they need real hardware/fixtures)

```bash
cd src-tauri
cargo test --lib audio -- --ignored --nocapture          # WASAPI devices, process loopback, mixer
CAPTURAX_FIXTURE=<dir> cargo test --lib mux_fixture -- --ignored   # needs avcc.bin + frames.bin
CAPTURAX_FIXTURE=<dir> cargo test --lib serve_fixture -- --ignored # serves <dir>/hls on :18787
```

## TypeScript Strictness
- Strict mode + `noUnusedLocals` + `noUnusedParameters` enabled
- All new code must be fully typed; avoid `any`

## Resolution & Framerate Data
- 48 standard resolutions in [`src/core/utils/constants.ts`](src/core/utils/constants.ts) (8K → QVGA), each with `tag`, `width`, `height`, `color`
- `Resolution` utility class: `getFromTag()`, `getApproximateTag()`, `getAspectRatio()`
- Framerates: `[320, 240, 180, 165, 144, 120, 60, 30, 25, 24]`
