<div align="center">
  <img src="assets/logo.png" alt="hyprwhspr-rs logo" width="200" />
  <h3>hyprwhspr-rs</h3>
  <p>Rust implementation of <a href="https://github.com/goodroot/hyprwhspr">hyprwhspr</a> | Native speech-to-text voice dictation for Hyprland.</p>
</div>
<hr />

> ⚠️ **Experimental:** This application is highly opinionated, and its defaults may not work for you. I am exploring more ergonomic defaults... expect breaking changes.

# Requirements

- whisper.cpp ([GitHub](https://github.com/ggml-org/whisper.cpp), [AUR](https://aur.archlinux.org/packages/whisper.cpp))

# Built for Hyprland

- Detects Hyprland via `HYPRLAND_INSTANCE_SIGNATURE` and opens the IPC socket at `$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock`.
- Execs `dispatch sendshortcut` commands against the active window to paste dictated text, inspecting `activewindow` to decide when `Shift` is required for a hardcoded list of programs.
- Falls back to a Wayland virtual keyboard client or a simulated keypress paste if IPC communication fails.

# Example Configuration

```jsonc
{
  "shortcuts": {
    "press": "SUPER+ALT+D",
    "hold": "SUPER+ALT+CTRL",
  },
  "stt_backend": "local", // Speech-to-text backend: "local" (default) or "groq"
  "model": "large-v3-turbo-q8_0", // Whisper model to use (must exist in specified directories)
  "models_dirs": [
    "~/.config/hyprwhspr-rs/models"
  ], // Directories to search for models
  "fallback_cli": false, // Fallback to whisper-cli (uses CPU)
  "threads": 4, // CPU threads to dedicate to transcription when using whisper-cli
  "word_overrides": {
    "under score": "_",
    "em dash": "—",
    "equal": "=",
    "at sign": "@",
    "pound": "#",
    "hashtag": "#",
    "hash tag": "#",
    "newline": "\n",
    "Omarkey": "Omarchy",
    "dot": ".",
    "Hyperland": "hyprland",
    "hyperland": "hyprland",
  },
  // Prompt text passed to Whisper for additional context.
  "whisper_prompt": "Transcribe as technical documentation with proper capitalization, acronyms, and technical terminology. Do not add punctuation.",
  "audio_feedback": true, // Play start/stop sounds while recording
  "start_sound_volume": 0.1, // 0.1 - 1.0
  "stop_sound_volume": 0.1, // 0.1 - 1.0
  "start_sound_path": null, // Optional custom audio asset overrides
  "stop_sound_path": null, // Optional custom audio asset overrides
  "auto_copy_clipboard": true, // Automatically copy the final transcription to the clipboard
  "shift_paste": false, // Whether to force shift paste
  "audio_device": null, // Force a specific input device index (null uses system default)
  "gpu_layers": 999, // Number of layers to keep on GPU (999 = auto/GPU preferred)
  "no_speech_threshold": 0.6, // Whisper's built-in "no speech" confidence gate (higher = more aggressive about returning empty text)
  "vad": {
    "enabled": false, // Toggles Silero VAD inside whisper.cpp
    "model": "ggml-silero-v5.1.2.bin", // Path or filename for the ggml Silero VAD model (ggml-silero-v5.1.2.bin)
    // Probability threshold for deciding a frame is speech. Higher = fewer false positives, but may miss quiet speech.
    "threshold": 0.5,
    // Minimum contiguous speech duration (ms) to accept. Increase to ignore quick clicks/taps.
    "min_speech_ms": 250,
    // Minimum silence gap (ms) required to end a speech segment. Raise if mid-sentence pauses are being split.
    "min_silence_ms": 120,
    // Maximum speech duration (seconds) before forcing a cut. Use Infinity to leave unlimited.
    "max_speech_s": 15.0,
    // Extra padding (ms) added before/after detected speech so words aren't clipped.
    "speech_pad_ms": 80,
    // Overlap ratio between segments (seconds). Higher overlap helps smooth transitions at the cost of a little extra decode time.
    "samples_overlap": 0.1,
  },
}
```

## Choosing a speech-to-text backend

hyprwhspr-rs now supports pluggable transcription backends. The default is the local
`whisper.cpp` integration (`stt_backend: "local"`). To use Groq's hosted Whisper service:

1. Obtain an API key from [console.groq.com](https://console.groq.com/).
2. Export it as `GROQ_API_KEY` in your environment.
3. Start the application with either the CLI flag or a config override:

   ```bash
   GROQ_API_KEY=... ./target/release/hyprwhspr-rs --groq
   ```

   or set `"stt_backend": "groq"` in your config file. CLI flags take precedence over the
   persisted configuration.

The Groq backend submits audio as an in-memory WAV via
`POST https://api.groq.com/openai/v1/audio/transcriptions` with `response_format=json` and
reads the `text` field from the response. Both backends return the same transcription string
to the rest of the application, so switching between them does not affect downstream behavior.

### Feature parity

- Local mode continues to support VAD, model selection, and GPU settings.
- Groq mode currently uses the `whisper-large-v3` model by default.
- Future enhancements (timeouts, retries, configurable model id, optional language hints, and
  Groq telemetry) are tracked as TODOs in the source.

# Development

1. `git clone https://github.com/better-slop/hyprwhispr-rs.git`
2. `cd hyprwhspr-rs`
3. `cargo build --release`
4. Run using:
    - Nice logs with pretty text transformation diffs: `RUST_LOG=debug ./target/release/hyprwhspr-rs`
    - Production release `./target/release/hyprwhspr-rs`
