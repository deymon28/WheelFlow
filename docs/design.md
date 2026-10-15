# Design notes

This document records the decisions behind WheelFlow's input pipeline, motion model, and frame timing, and the invariants future changes must preserve.

## Input pipeline

- **Hook thread.** A low-level mouse hook runs on its own message-loop thread at raised priority. Windows silently removes a low-level hook whose callback exceeds `LowLevelHooksTimeout`, so the callback does no process lookup, file IO, UI work, blocking lock acquisition, or input injection. It reads a cached per-window policy with `try_read` and hands accepted wheel events to the worker through a bounded `try_send` queue. If the queue is full, the event passes through natively.
- **Policy prefetch.** On pointer movement, at most every 25 ms, the hook finds the top-level window under the pointer and asks the worker to classify it if its policy is missing or ageing. The first wheel event over a new window is therefore usually smoothed. Unknown windows keep native input until classified; ageing verdicts stay valid while they refresh. Because movement handling is throttled, a 100 ms tick on the hook thread repeats the check after the pointer stops. A hover change is recorded only after the worker has accepted it, so a full queue cannot lose it.
- **Policies.** The worker resolves eligibility: excluded process names, integrity level (a non-elevated process cannot inject into elevated windows), fullscreen documents versus games, and Explorer item views. A rejected injection bypasses only that window for two seconds.
- **Watchdog.** Every two seconds, the hook thread checks whether the pointer moved while the hook saw no mouse events. If so, it reinstalls the hook. A non-elevated hook never receives input while an elevated window is in the foreground, so that silence is not counted. Log lines are limited to one per minute.
- **Cancellation.** Reversal, modifiers, target changes, and disabling increment an epoch. Queued events from an older epoch are discarded, and every output piece rechecks the epoch, modifiers, and target before it is sent.

## Motion model

Each wheel event becomes an independent impulse with its own finite timeline. Overlapping impulses add linearly, so a new notch never restarts or speeds up earlier motion.

The velocity of an impulse of duration `T` is proportional to `e^(−t/τd) − e^(−t/τr)`, with `τd = T/5` and `τr = min(3 ms, τd/4)`. It is integrated in closed form and normalized to end exactly at `T`. Velocity peaks within a few milliseconds and then decays exponentially, so the first displayed frame already moves without a one-frame jump. At `T = 250 ms`, a 300 Hz display shows about 3% of the distance on the first frame and 29% after 20 ms; 90% is covered near 115 ms.

Distance is integrated in floating point and quantized only at the output. The residual is rounded as a running sum, so fractional input carries across gestures. A reversal discards unissued momentum.

**Acceleration.** Same-direction input closer than 60 ms in notch-equivalent spacing (`gap × 120 / |delta|`) is multiplied by `(1 + 60 / spacing) / 2`, capped at 3. The spacing is averaged with a weight proportional to wheel distance, so high-resolution and notched wheels behave alike. The first notch of a gesture and every isolated notch travel exactly speed × delta.

**Bounded storage.** Each axis keeps at most 512 impulses. Under extreme pressure, the newest remaining distance is merged into the incoming impulse rather than dropped.

## Frame timing

Output is synchronized to the monitor that shows the target window. A dedicated thread waits for that monitor's vertical blanks (`D3DKMTOpenAdapterFromGdiDisplayName` and `D3DKMTWaitForVerticalBlankEvent`) while scrolling and for two seconds afterwards. It refines the refresh period with a slow average and ignores waits that return implausibly fast, which some drivers do while a display is off.

The worker sends one message per axis per refresh, a quarter frame after the vertical blank, on a high-resolution waitable timer. Motion is evaluated at the slot time rather than the wake-up time. Timer wake-ups are quantized (often to 0.5 ms), but the steps between displayed frames stay equal. Missed slots are skipped instead of sent back to back. Before the first vertical blank of a monitor is observed, output uses its nominal refresh rate and starts immediately.

A fixed cadence, or one taken from a different monitor, gives some frames more steps than others. On a 100 Hz display fed at 300 Hz, frames receive 2, 3, or 4 steps, which is visible as judder.

The hook, vertical-blank, and output threads use raised thread priorities, and the process opts out of Windows power throttling so background timers stay precise. There is no busy waiting, global timer-resolution change, or real-time process priority.

## Application compatibility

- **Chromium.** Chromium-based browsers and Electron apps treat wheel deltas that are exact multiples of `WHEEL_DELTA` (120) as coarse notches and animate them with their own smooth scrolling. Other values scroll precisely ([Chromium change](https://chromium.googlesource.com/chromium/src/+/3c807168bd0999858f609c4ab3fad5feb135b329)). An animated frame whose delta would be a nonzero multiple of 120 sends one unit less and carries the unit forward.
- **Explorer item views.** File Explorer windows (`CabinetWClass` in `explorer.exe`) scroll a fixed number of lines per accumulated `WHEEL_DELTA` and ignore partial progress. While the pointer is over such a window, the session's `SPI_SETWHEELSCROLLLINES` value is set to 1 with `SPIF_SENDCHANGE` and output is scaled by the user's line count. Distance stays the same and steps become a single line. Rules for this mode:
  - The saved profile value is never written (`SPIF_UPDATEINIFILE` is not used).
  - The session value is restored when the pointer leaves, when the target changes, when WheelFlow is disabled, and at exit.
  - On the next start after an unclean exit, a session value of 1 with a larger saved value is restored.
  - Changes run on their own thread, because the broadcast can block.
  - Output is scaled only after the change has been applied.
- **Fullscreen.** When the pointer is confined, or the target is a captionless, non-maximized top-level window covering its monitor, input passes through unchanged. Browsers, UWP frames, Explorer, and the desktop are excepted. Games often read smoothed output through raw input as many separate notches.
- **Elevation.** Windows rejects input injected into higher-integrity windows. The optional administrator mode relaunches WheelFlow through `ShellExecuteEx` with the `runas` verb. The new instance waits up to five seconds for the previous one to release the single-instance mutex. An elevated instance accepts `WM_CLOSE` from lower integrity so `--stop` keeps working.
- **Large outputs.** Wheel messages carry signed 16-bit deltas, so large overdue frames are split into pieces of at most 32767 units.

## Startup, instances, and resources

- **Single instance.** A session-local mutex guards the tray instance. A manual second launch posts a message asking the running instance to show its settings, and grants it the foreground first. A start with `--autostart` exits quietly. An instance restarted for elevation waits up to five seconds for the previous one to exit.
- **Start with Windows.** A normal instance writes `"<exe>" --autostart` to the per-user Run key. When administrator mode is also enabled, an elevated instance registers a Task Scheduler logon task instead (`schtasks /Create /XML`) and removes the Run value. The task uses the highest available run level, normal priority (4), no execution time limit, and no battery stop. A normal instance cannot create or delete that task, so it writes the Run value until the next elevated start replaces it. The registration is refreshed at startup when autostart is on or the instance is elevated, which also corrects a moved executable.
- **Resources.** `build.rs` writes a `.res` file directly and passes it to the MSVC linker, so no resource compiler or crate is needed. The file contains:
  - an icon group in eight sizes, drawn by `native/icon.rs`, which also draws the enabled and paused tray icons at run time;
  - version information;
  - a manifest that enables Common Controls 6 (themed settings controls), per-monitor DPI awareness v2, Windows 10/11 compatibility, and `asInvoker` execution.

## Verification

`test.ps1` runs unit tests for the curve, acceleration, frame slots, configuration, and Win32 ABI layouts, then exercises the real hook and `SendInput` path in `--verify` mode. A separate observer hook swallows all marked test input and output, so nothing reaches applications. Verification never changes system settings and never shows elevation prompts. The checks cover:

- delivery of fine and large wheel deltas;
- first-output latency;
- acceleration bounds;
- one output per refresh on every connected monitor, with no animated 120-multiple frames;
- bursts and delayed large frames;
- a stalled UI thread;
- modifier and backlog cancellation;
- temporary delivery failure;
- disabling and re-enabling.

Physical-mouse feel, application rendering, the Explorer line mode, and elevation require hands-on checks.
