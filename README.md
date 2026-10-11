# WheelFlow 0.5.0

Small native Windows 11 x64 tray utility for smooth mouse-wheel scrolling. One EXE, no .NET, driver, network access, or automatic startup. The C runtime is statically linked; only Windows system DLLs are required to run it.

## Use

Launch `bin\WheelFlow.exe`. Right-click its tray icon for **Enabled**, **Settings**, **Diagnostics**, or **Exit**. Double-click the icon to open settings. `bin\WheelFlow.exe --stop` closes the running instance gracefully, including an elevated one.

Preferences live in `%APPDATA%\WheelFlow\settings.ini`; existing files are preserved and new keys get defaults. Settings:

- **Scroll speed** (50-200%): distance multiplier. 100% scrolls the same distance as Windows.
- **Glide duration** (60-600 ms, default 250): how long each notch keeps gliding. Motion starts on the first displayed frame; about 90% of it is done after 45% of the duration.
- **Accelerate fast wheel spins** (default on): quick consecutive notches travel further, up to 3x. Slow or isolated notches always travel their normal distance.
- **Leave fullscreen games and players unsmoothed** (default on): wheel input passes through unchanged while a captionless window covers its monitor (browsers and Explorer excepted) or an application confines the pointer.
- **Run as administrator** (default off): Windows does not let a normal program send input to elevated windows, such as a terminal opened as administrator. With this option WheelFlow restarts itself with a UAC prompt, and asks again on each start; a declined prompt keeps it running without elevation.
- **Excluded apps**: process names that always receive ordinary wheel input.

Diagnostics shows whether WheelFlow is elevated, the output frame rate of the last target, counters, and maximum callback duration; bounded error/startup logs are in `%LOCALAPPDATA%\WheelFlow\diagnostics.log`. Logs contain no recorded mouse movements or keystrokes.

## Behavior

- Output is synchronized to the vertical blank of the monitor showing the target window (for example 300 Hz on one display and 100 Hz on another): one wheel message per displayed frame, sent a quarter frame after the vertical blank, each carrying an equal step. Motion is evaluated at the frame slot, not the wake-up time, so timer jitter never becomes uneven movement.
- Each wheel event has its own finite glide: velocity peaks within a few milliseconds and then decays exponentially, so the first frame already moves without a visible jump. Overlapping notches add linearly without restarting earlier ones.
- Animated frames never carry an exact multiple of 120 units. Chromium-based browsers and apps (Chrome, Edge, Electron) treat such deltas as coarse notches and animate them a second time; other values scroll precisely.
- File Explorer windows scroll whole lines per accumulated notch and ignore partial progress. While the pointer is over an Explorer window, WheelFlow sets the session's "lines per notch" to 1 and scales its output by the user's value, so distance is unchanged and Explorer moves in single-line steps. The saved preference is never written; it is restored when the pointer leaves, when WheelFlow exits, or on the next start after a crash.
- All nonzero physical wheel deltas are supported, including high-resolution wheels and strong bursts. Fractional distance carries across gestures.
- Reversing the wheel immediately cancels outstanding motion in the previous direction. Changing target windows, pausing, or pressing modifiers also cancels outstanding motion, including queued events.
- Ctrl, Shift, Alt, and Win plus wheel pass through. Animation rechecks modifiers before delivery, preventing a scroll tail from becoming zoom.
- The window under the pointer is classified while the pointer moves, so the first wheel event over a new window is already smoothed. Excluded applications, and elevated ones unless WheelFlow runs as administrator, receive ordinary input.
- A watchdog reinstalls the hook if Windows silently removes it (for example after a system-wide stall exceeded `LowLevelHooksTimeout`).
- A rejected injection temporarily bypasses only its target. It does not change the Enabled preference. A failed piece may be lost.
- Disable other scrolling/remapping utilities while evaluating this one.

## Build and verify

Install the Rust x64 MSVC toolchain and Visual Studio C++ build tools. No Rust crates need downloading.

```powershell
powershell -ExecutionPolicy Bypass -File .\build.ps1
powershell -ExecutionPolicy Bypass -File .\test.ps1
```

The second command runs deterministic tests, builds the release EXE, and verifies the real Win32 hook/SendInput path. A separate observer swallows all marked test input and output before it reaches applications; it does not move the pointer or press keys, and verification never changes system settings. It covers wheel sizes, first-output latency, acceleration bounds, vertical-blank synchronization on every connected monitor, absence of 120-multiple frames, strong bursts, delayed large frames, a busy UI thread, cancellation of backlog, temporary delivery failure, and re-enabling. The report is `bin\verification.txt`. Physical-mouse feel, the Explorer line mode, elevation, and application-specific rendering still need hands-on checking.

## Architecture

| File | Responsibility |
| --- | --- |
| `native/runtime.rs` | Hook thread with watchdog, policy prefetch, bounded input queue, target policies, Explorer line mode, cancellation, output worker |
| `native/engine.rs` | Glide impulses, acceleration, fractional wheel-distance accounting |
| `native/clock.rs` | Per-monitor vertical-blank observation thread and output slot scheduling, with a nominal-rate fallback |
| `native/ui.rs` | Native tray and settings controls, DPI handling, elevated restart |
| `native/config.rs` | Compatible INI preferences |
| `native/win.rs` | Win32 bindings, window and monitor classification, ABI checks |
| `native/verify.rs` | Marked-input observer and pipeline regression checks |

The hook callback performs no process lookup, file IO, blocking lock acquisition, UI work, or input injection; on pointer movement it only requests classification of a new window, at most every 25 ms. A dedicated thread waits for vertical blanks (`D3DKMTWaitForVerticalBlankEvent`) on the target's monitor while scrolling and for two seconds afterwards; the worker then sends output on a high-resolution waitable timer. Missed slots are skipped and timers stop when motion finishes. There is no busy wait, global timer-resolution change, or real-time process priority; the hook, vertical-blank, and output threads use raised thread priorities and the process opts out of Windows power throttling so background timers stay precise. Session wheel-line changes run on their own thread because they broadcast to every window. Each axis holds at most 512 impulses; extreme overload merges the newest remaining motion while preserving distance. Hit tests use physical coordinates. Large outputs are split to fit signed 16-bit wheel messages.

## Limits

Explorer and some other Windows controls still move in whole lines; WheelFlow makes those steps smaller but cannot make them pixel-smooth. Applications that treat every wheel message as a full notch regardless of its size should be added to the exclusions. Killing WheelFlow while it hovers Explorer leaves the session at one line per notch until WheelFlow starts again or the user signs out. See [design notes](docs/design.md) for the motion model, frame timing, compatibility decisions, and invariants.
