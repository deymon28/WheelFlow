# Changelog

## [0.6.0] - 2026-10-14

### Added

- **Start with Windows** option. A normal instance registers itself in the per-user Run key. With administrator mode, an elevated instance registers a logon task with the highest run level instead, so WheelFlow starts elevated without a UAC prompt at every sign-in.
- WheelFlow icon, drawn procedurally: a blue icon in the tray while enabled and a gray one while paused. The tooltip shows when WheelFlow runs as administrator.
- Embedded executable resources: icon, version information, and an application manifest (themed Windows controls, per-monitor DPI awareness, Windows 10/11 compatibility).
- Launching WheelFlow while it is already running opens the running instance's settings instead of showing an error. Automatic starts exit quietly when an instance is already running.
- GitHub Actions workflows: formatting, unit tests, and release build on every push, plus a release workflow that attaches `WheelFlow.exe` to version tags.

### Fixed

- The Explorer line mode is now restored even when the pointer leaves Explorer and stops moving within the movement throttle. A 100 ms tick on the hook thread settles the hover state. A hover change is recorded only after the worker has been told about it.
- The hook watchdog no longer reinstalls the hook every few seconds while an elevated window is in the foreground. Windows never passes that input to a non-elevated hook. Watchdog log lines are limited to one per minute.
- Unit tests that read display and registry state tolerate headless build machines.
- Compiler warnings are cleaned up.

## [0.5.0] - 2026-10-11

First public release: low-level hook with watchdog and policy prefetch, per-monitor vertical-blank synchronized output, finite overlapping glide impulses with bounded acceleration, Chromium-friendly deltas, Explorer line mode, fullscreen pass-through, optional elevation, and a verification mode that exercises the real Win32 input path without reaching applications.
