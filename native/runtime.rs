use crate::{
    clock::{FrameClock, Vsync},
    config::Config,
    engine::{Axis, Profile},
    win::*,
};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    ptr::null,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, OnceLock, RwLock,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
const POLICY_LIFETIME_MS: f64 = 30000.0;
const POLICY_REFRESH_MS: f64 = 5000.0;
const PREFETCH_INTERVAL_US: u64 = 25000;
// The hook thread's timer settles the hover state after the pointer stops
// (movement handling is throttled) and runs the hook watchdog every 2 s.
const TICK_MS: u32 = 100;
const WATCHDOG_TICKS: u32 = 20;
// Windows controls whose fullscreen mode is still a document, not a game.
const FULLSCREEN_DOCUMENTS: [&str; 7] = [
    "Chrome_WidgetWin_0",
    "Chrome_WidgetWin_1",
    "MozillaWindowClass",
    "ApplicationFrameWindow",
    "CabinetWClass",
    "Progman",
    "WorkerW",
];
// Explorer's item views scroll a fixed number of lines per accumulated
// WHEEL_DELTA and ignore partial progress. While such a window is hovered,
// the session's wheel lines are set to 1 and output is scaled by the user's
// line count instead, which keeps the distance and triples the granularity.
const LINE_SCROLLERS: [&str; 1] = ["CabinetWClass"];

#[derive(Clone, Copy)]
struct Policy {
    pid: u32,
    allowed: bool,
    fullscreen_document: bool,
    line_scroller: bool,
    // A delivery failure bypasses the target briefly instead of caching a verdict.
    temporary: bool,
    expires: f64,
}
#[derive(Clone, Copy)]
struct Wheel {
    delta: i32,
    horizontal: bool,
    root: Handle,
    at: f64,
    epoch: u64,
}
enum Command {
    Wheel(Wheel),
    Resolve(Handle, u32),
    Hover(Handle),
    Configure(Config),
    Stop,
}

#[derive(Default)]
pub struct Stats {
    pub captured: AtomicU64,
    pub emitted: AtomicU64,
    pub bypassed: AtomicU64,
    pub fullscreen: AtomicU64,
    pub failures: AtomicU64,
    pub queue_full: AtomicU64,
    pub max_hook_us: AtomicU64,
    pub installed: AtomicU64,
    pub watchdog: AtomicU64,
    pub frame_period_us: AtomicU64,
    pub synchronized: AtomicBool,
    pub last_error: AtomicU32,
}

pub struct Shared {
    sender: SyncSender<Command>,
    wake: Handle,
    clock: Instant,
    policies: RwLock<HashMap<Handle, Policy>>,
    requested: AtomicIsize,
    cancel_epoch: AtomicU64,
    hook_thread: AtomicU32,
    hook_events: AtomicU64,
    prefetch_at: AtomicU64,
    hovered: AtomicIsize,
    fullscreen_bypass: AtomicBool,
    vsync: Arc<Vsync>,
    lines: LineSetting,
    pub enabled: AtomicBool,
    pub stats: Stats,
    pub verifying: bool,
    pub test_modifiers: AtomicBool,
    pub test_fail_next: AtomicU32,
    pub test_hold_worker: AtomicBool,
    pub test_monitor: AtomicIsize,
}

impl Shared {
    pub fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1000.0
    }
    fn now_us(&self) -> u64 {
        self.clock.elapsed().as_micros() as u64
    }
    fn notify(&self) {
        unsafe {
            SetEvent(self.wake);
        }
    }
    pub fn cancel_pending(&self) {
        self.cancel_epoch.fetch_add(1, Ordering::Relaxed);
        self.notify();
    }
    pub fn update(&self, config: Config) {
        self.enabled.store(config.enabled, Ordering::Release);
        self.fullscreen_bypass
            .store(config.fullscreen_bypass, Ordering::Relaxed);
        self.cancel_epoch.fetch_add(1, Ordering::Relaxed);
        // Only the UI may wait for queue space; the input callback always uses try_send.
        let _ = self.sender.send(Command::Configure(config));
        self.notify();
        unsafe {
            PostThreadMessageW(self.hook_thread.load(Ordering::Relaxed), WM_REHOOK, 0, 0);
        }
    }
    pub fn diagnostics(&self) -> String {
        let s = &self.stats;
        let period = s.frame_period_us.load(Ordering::Relaxed);
        let refresh = if period == 0 {
            "not measured yet".to_owned()
        } else {
            format!(
                "{:.1} Hz ({})",
                1e6 / period as f64,
                if s.synchronized.load(Ordering::Relaxed) {
                    "display-synchronized"
                } else {
                    "fallback timer"
                }
            )
        };
        format!("WheelFlow {} (native Rust)\nEnabled: {}\nAdministrator: {}\nOutput frame rate (last target): {}\nCaptured wheel events: {}\nEmitted pieces: {}\nPassed through: {}\n  of which fullscreen/confined: {}\nTemporary delivery failures: {}\nQueue overflow (passed through): {}\nMaximum callback work: {} us\nHook installations: {}\n  of which by watchdog: {}\nLast Win32 delivery error: {}\n\nLog: %LOCALAPPDATA%\\WheelFlow\\diagnostics.log",
            env!("CARGO_PKG_VERSION"),
            self.enabled.load(Ordering::Relaxed),
            if running_elevated() { "yes (elevated windows are smoothed)" } else { "no (elevated windows pass through)" },
            refresh, s.captured.load(Ordering::Relaxed), s.emitted.load(Ordering::Relaxed), s.bypassed.load(Ordering::Relaxed), s.fullscreen.load(Ordering::Relaxed), s.failures.load(Ordering::Relaxed), s.queue_full.load(Ordering::Relaxed), s.max_hook_us.load(Ordering::Relaxed), s.installed.load(Ordering::Relaxed), s.watchdog.load(Ordering::Relaxed), s.last_error.load(Ordering::Relaxed))
    }
    fn modifiers(&self) -> bool {
        if self.verifying {
            self.test_modifiers.load(Ordering::Relaxed)
        } else {
            modifiers()
        }
    }
    fn request(&self, root: Handle, pid: u32) {
        if self.requested.swap(root, Ordering::Relaxed) != root {
            if self.sender.try_send(Command::Resolve(root, pid)).is_ok() {
                self.notify();
            } else {
                self.requested.store(0, Ordering::Relaxed);
            }
        }
    }
    fn log(&self, message: &str) {
        if self.verifying {
            return;
        }
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            let folder = std::path::PathBuf::from(base).join("WheelFlow");
            let _ = std::fs::create_dir_all(&folder);
            let path = folder.join("diagnostics.log");
            // Rotation is bounded; input content and mouse coordinates are never logged.
            if std::fs::metadata(&path)
                .map(|m| m.len() > 65536)
                .unwrap_or(false)
            {
                let _ = std::fs::rename(&path, folder.join("diagnostics.previous.log"));
            }
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = writeln!(
                    file,
                    "{} UTC(epoch) +{:.0}ms {}",
                    stamp,
                    self.now(),
                    message
                );
            }
        }
    }
}

/// Temporary session changes of the wheel-lines value for line scrollers.
/// SPIF_SENDCHANGE broadcasts to every top-level window, so changes run on
/// their own thread and never stall output frames. The saved preference in the
/// user profile is never written, so a crash cannot make the change permanent.
struct LineSetting {
    sender: std::sync::Mutex<Option<mpsc::Sender<u32>>>,
    thread: std::sync::Mutex<Option<JoinHandle<()>>>,
    original: Arc<AtomicU32>,
    applied: Arc<AtomicBool>,
}

impl LineSetting {
    fn new(enabled: bool) -> Self {
        let original = Arc::new(AtomicU32::new(0));
        let applied = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel::<u32>();
        let thread = enabled.then(|| {
            let (original, applied) = (original.clone(), applied.clone());
            thread::spawn(move || {
                for lines in receiver {
                    let ok = set_wheel_scroll_lines(lines);
                    applied.store(
                        ok && lines == 1 && original.load(Ordering::Acquire) != 0,
                        Ordering::Release,
                    );
                }
            })
        });
        Self {
            sender: std::sync::Mutex::new(enabled.then_some(sender)),
            thread: std::sync::Mutex::new(thread),
            original,
            applied,
        }
    }

    fn send(&self, lines: u32) {
        if let Ok(sender) = self.sender.lock() {
            if let Some(sender) = sender.as_ref() {
                let _ = sender.send(lines);
            }
        }
    }

    fn enter(&self) {
        if self.original.load(Ordering::Acquire) != 0 {
            return;
        }
        if let Some(lines) = wheel_scroll_lines().filter(|l| (2..=100).contains(l)) {
            self.original.store(lines, Ordering::Release);
            self.send(1);
        }
    }

    fn leave(&self) {
        let original = self.original.swap(0, Ordering::AcqRel);
        if original != 0 {
            self.applied.store(false, Ordering::Release);
            self.send(original);
        }
    }

    /// Output multiplier keeping line-scroller distance while lines are 1.
    fn factor(&self) -> u32 {
        let original = self.original.load(Ordering::Acquire);
        if original != 0 && self.applied.load(Ordering::Acquire) {
            original
        } else {
            1
        }
    }

    fn shutdown(&self) {
        self.leave();
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        if let Some(thread) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = thread.join();
        }
    }
}

/// Repairs a session value left at 1 by an earlier instance that did not exit cleanly.
fn restore_abandoned_lines(state: &Shared) {
    if let (Some(1), Some(saved)) = (wheel_scroll_lines(), saved_wheel_scroll_lines()) {
        if saved > 1 && set_wheel_scroll_lines(saved) {
            state.log(&format!(
                "restored wheel scroll lines to the saved value {saved}"
            ));
        }
    }
}

pub struct Runtime {
    pub shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    hook: Option<JoinHandle<()>>,
}

impl Runtime {
    pub fn start(config: Config, verifying: bool) -> Result<Self, String> {
        let wake = unsafe { CreateEventW(null(), 0, 0, null()) };
        if wake == 0 {
            return Err("Could not create input notification event".into());
        }
        let (sender, receiver) = mpsc::sync_channel(2048);
        let clock = Instant::now();
        let shared = Arc::new(Shared {
            sender,
            wake,
            clock,
            policies: RwLock::new(HashMap::new()),
            requested: AtomicIsize::new(0),
            cancel_epoch: AtomicU64::new(0),
            hook_thread: AtomicU32::new(0),
            hook_events: AtomicU64::new(0),
            prefetch_at: AtomicU64::new(0),
            hovered: AtomicIsize::new(0),
            fullscreen_bypass: AtomicBool::new(config.fullscreen_bypass),
            vsync: Vsync::start(clock),
            // Verification never changes system settings.
            lines: LineSetting::new(!verifying),
            enabled: AtomicBool::new(config.enabled),
            stats: Stats::default(),
            verifying,
            test_modifiers: AtomicBool::new(false),
            test_fail_next: AtomicU32::new(0),
            test_hold_worker: AtomicBool::new(false),
            test_monitor: AtomicIsize::new(0),
        });
        SHARED
            .set(shared.clone())
            .map_err(|_| "Runtime already initialized")?;
        if !verifying {
            restore_abandoned_lines(&shared);
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("scroll-output".into())
            .spawn(move || worker_loop(state, receiver, config, ready_tx))
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        let (ready_tx, ready_rx) = mpsc::channel();
        let state = shared.clone();
        let hook = thread::Builder::new()
            .name("mouse-hook".into())
            .spawn(move || hook_loop(state, ready_tx))
            .map_err(|e| e.to_string())?;
        if let Err(error) = ready_rx.recv().map_err(|e| e.to_string())? {
            let _ = shared.sender.send(Command::Stop);
            shared.notify();
            let _ = worker.join();
            let _ = hook.join();
            unsafe {
                CloseHandle(wake);
            }
            return Err(error);
        }
        shared.log(&format!(
            "ready {}: hook thread with watchdog; display-synchronized output only while scrolling",
            env!("CARGO_PKG_VERSION")
        ));
        Ok(Self {
            shared,
            worker: Some(worker),
            hook: Some(hook),
        })
    }

    pub fn stop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        self.shared.enabled.store(false, Ordering::Release);
        let _ = self.shared.sender.send(Command::Stop);
        self.shared.notify();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        unsafe {
            PostThreadMessageW(
                self.shared.hook_thread.load(Ordering::Relaxed),
                WM_QUIT,
                0,
                0,
            );
        }
        if let Some(hook) = self.hook.take() {
            let _ = hook.join();
        }
        self.shared.lines.shutdown();
        self.shared.vsync.shutdown();
        self.shared.log("stopped");
        unsafe {
            CloseHandle(self.shared.wake);
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn install_hook() -> Handle {
    unsafe { SetWindowsHookExW(14, Some(mouse_hook), GetModuleHandleW(null()), 0) }
}

fn hook_loop(state: Arc<Shared>, ready: mpsc::Sender<Result<(), String>>) {
    unsafe {
        SetThreadDpiAwarenessContext(-4);
        // The thread sleeps in GetMessage; when input arrives it must run before
        // busy applications so the system-wide hook never approaches its timeout.
        set_thread_priority(15);
        let mut message = Msg::default();
        PeekMessageW(&mut message, 0, 0, 0, 0);
        state
            .hook_thread
            .store(GetCurrentThreadId(), Ordering::Relaxed);
        let mut handle = install_hook();
        if handle == 0 {
            let _ = ready.send(Err(format!(
                "Could not install mouse hook: {}",
                GetLastError()
            )));
            return;
        }
        state.stats.installed.fetch_add(1, Ordering::Relaxed);
        let _ = ready.send(Ok(()));
        if !state.verifying {
            SetTimer(0, 0, TICK_MS, null());
        }
        let mut last_point = Point::default();
        GetPhysicalCursorPos(&mut last_point);
        let mut last_seen = state.hook_events.load(Ordering::Relaxed);
        let mut ticks = 0u32;
        let mut last_logged: Option<f64> = None;
        while GetMessageW(&mut message, 0, 0, 0) > 0 {
            if message.message == WM_TIMER {
                let mut point = Point::default();
                if GetPhysicalCursorPos(&mut point) != 0 {
                    classify(&state, point, state.now_us());
                }
                ticks += 1;
            }
            let reinstall = match message.message {
                WM_REHOOK => true,
                WM_TIMER if ticks % WATCHDOG_TICKS == 0 => {
                    // Windows silently removes a low-level hook that once exceeded
                    // LowLevelHooksTimeout. If the pointer moved but the hook saw
                    // nothing, reinstall it instead of staying unsmoothed.
                    let seen = state.hook_events.load(Ordering::Relaxed);
                    let mut point = Point::default();
                    let moved = GetPhysicalCursorPos(&mut point) != 0
                        && (point.x, point.y) != (last_point.x, last_point.y);
                    // Input to an elevated foreground window never reaches a
                    // non-elevated hook; that silence is expected, not a loss.
                    let lost = moved
                        && seen == last_seen
                        && (running_elevated() || !foreground_elevated());
                    last_point = point;
                    last_seen = seen;
                    if lost {
                        state.stats.watchdog.fetch_add(1, Ordering::Relaxed);
                        // Bounded log: at most one line per minute.
                        let now = state.now();
                        if last_logged.map(|at| now - at >= 60000.0).unwrap_or(true) {
                            last_logged = Some(now);
                            state.log("mouse hook stopped receiving input; reinstalling");
                        }
                    }
                    lost
                }
                _ => false,
            };
            if reinstall {
                let replacement = install_hook();
                if replacement != 0 {
                    UnhookWindowsHookEx(handle);
                    handle = replacement;
                    state.stats.installed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        UnhookWindowsHookEx(handle);
    }
}

unsafe extern "system" fn mouse_hook(code: i32, message: usize, data: isize) -> isize {
    if code < 0 || data == 0 {
        return CallNextHookEx(0, code, message, data);
    }
    let Some(state) = SHARED.get() else {
        return CallNextHookEx(0, code, message, data);
    };
    state.hook_events.fetch_add(1, Ordering::Relaxed);
    let event = &*(data as *const MouseHook);
    if message == MOUSEMOVE {
        if !state.verifying {
            prefetch(state, event.point);
        }
        return CallNextHookEx(0, code, message, data);
    }
    if message != WHEEL && message != HWHEEL {
        return CallNextHookEx(0, code, message, data);
    }
    let accepted_source = if state.verifying {
        event.extra == TEST_RAW
    } else {
        event.flags & 1 == 0
    };
    if !accepted_source {
        return CallNextHookEx(0, code, message, data);
    }
    let start = Instant::now();
    let captured = capture(state, event, message == HWHEEL);
    state
        .stats
        .max_hook_us
        .fetch_max(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    if captured {
        1
    } else {
        CallNextHookEx(0, code, message, data)
    }
}

// Resolve the policy of the window under the pointer before the first wheel
// event reaches it, so that event is smoothed instead of passed through.
fn prefetch(state: &Shared, point: Point) {
    if !state.enabled.load(Ordering::Relaxed) {
        return;
    }
    let now = state.now_us();
    if now.saturating_sub(state.prefetch_at.load(Ordering::Relaxed)) < PREFETCH_INTERVAL_US {
        return;
    }
    state.prefetch_at.store(now, Ordering::Relaxed);
    classify(state, point, now);
}

// Requests classification of the window at `point` when needed, and reports
// hover changes so per-window system adjustments follow the pointer.
fn classify(state: &Shared, point: Point, now: u64) {
    if !state.enabled.load(Ordering::Relaxed) {
        return;
    }
    let root = root_at(point);
    if root == 0 {
        return;
    }
    let pid = process_id(root);
    let at = now as f64 / 1000.0;
    let fresh = state
        .policies
        .try_read()
        .ok()
        .and_then(|cache| cache.get(&root).copied())
        .map(|p| {
            p.pid == pid
                && if p.temporary {
                    at < p.expires
                } else {
                    at < p.expires - POLICY_REFRESH_MS
                }
        })
        .unwrap_or(false);
    if !fresh {
        state.request(root, pid);
    }
    // Record the hover only once the worker has been told, so a full queue
    // is retried on the next movement or tick instead of being forgotten.
    if state.hovered.load(Ordering::Relaxed) != root
        && state.sender.try_send(Command::Hover(root)).is_ok()
    {
        state.hovered.store(root, Ordering::Relaxed);
        state.notify();
    }
}

fn capture(state: &Shared, event: &MouseHook, horizontal: bool) -> bool {
    let delta = (event.data >> 16) as u16 as i16 as i32;
    if !state.enabled.load(Ordering::Acquire) || delta == 0 {
        return false;
    }
    if state.modifiers() {
        state.cancel_pending();
        return false;
    }
    let at = state.now();
    let root;
    if state.verifying {
        root = 1;
        let allowed = state
            .policies
            .try_read()
            .ok()
            .and_then(|cache| cache.get(&root).copied())
            .map(|p| p.allowed || at >= p.expires)
            .unwrap_or(true);
        if !allowed {
            state.stats.bypassed.fetch_add(1, Ordering::Relaxed);
            return false;
        }
    } else {
        let fullscreen_bypass = state.fullscreen_bypass.load(Ordering::Relaxed);
        if fullscreen_bypass && cursor_confined() {
            state.stats.bypassed.fetch_add(1, Ordering::Relaxed);
            state.stats.fullscreen.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        root = root_at(event.point);
        if root == 0 {
            return false;
        }
        let pid = process_id(root);
        let policy = state
            .policies
            .try_read()
            .ok()
            .and_then(|cache| cache.get(&root).copied());
        match policy {
            Some(policy) if policy.pid == pid && (!policy.temporary || at < policy.expires) => {
                // An aging verdict stays valid while the worker refreshes it.
                if !policy.temporary && at >= policy.expires - POLICY_REFRESH_MS {
                    state.request(root, pid);
                }
                if !policy.allowed {
                    state.stats.bypassed.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                if fullscreen_bypass && !policy.fullscreen_document && covers_monitor(root) {
                    state.stats.bypassed.fetch_add(1, Ordering::Relaxed);
                    state.stats.fullscreen.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
            _ => {
                // Unknown targets keep native input until the worker resolves them.
                state.request(root, pid);
                state.stats.bypassed.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        }
    }
    let epoch = state.cancel_epoch.load(Ordering::Relaxed);
    if state
        .sender
        .try_send(Command::Wheel(Wheel {
            delta,
            horizontal,
            root,
            at,
            epoch,
        }))
        .is_err()
    {
        state.stats.queue_full.fetch_add(1, Ordering::Relaxed);
        state.cancel_pending();
        return false;
    }
    state.stats.captured.fetch_add(1, Ordering::Relaxed);
    state.notify();
    true
}

fn resolve_policy(state: &Shared, config: &Config, root: Handle, pid: u32) {
    if root == 0 {
        return;
    }
    let mut explorer = false;
    let allowed = unsafe {
        let process = OpenProcess(0x1000, 0, pid);
        if process == 0 {
            false
        } else {
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let name_ok =
                QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0;
            let name = if name_ok {
                String::from_utf16_lossy(&buffer[..length as usize])
                    .rsplit('\\')
                    .next()
                    .unwrap_or("")
                    .to_lowercase()
            } else {
                String::new()
            };
            let excluded = config.exclusions.iter().any(|candidate| {
                candidate
                    .trim()
                    .trim_end_matches(".exe")
                    .eq_ignore_ascii_case(name.trim_end_matches(".exe"))
            });
            let privileges_ok = match (elevated(GetCurrentProcess()), elevated(process)) {
                (Some(own), Some(target)) => own || !target,
                _ => false,
            };
            CloseHandle(process);
            explorer = name == "explorer.exe";
            name_ok && !excluded && privileges_ok
        }
    };
    let class = class_name(root);
    let fullscreen_document = FULLSCREEN_DOCUMENTS.contains(&class.as_str());
    let line_scroller = explorer && LINE_SCROLLERS.contains(&class.as_str());
    if let Ok(mut cache) = state.policies.write() {
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(
            root,
            Policy {
                pid,
                allowed,
                fullscreen_document,
                line_scroller,
                temporary: false,
                expires: state.now() + POLICY_LIFETIME_MS,
            },
        );
    }
    state.requested.store(0, Ordering::Relaxed);
}

struct Motion {
    axes: [Axis; 2],
    target: Handle,
    monitor: Handle,
    next: Option<f64>,
    last_frame: Option<f64>,
    epoch: u64,
}
impl Motion {
    fn new() -> Self {
        Self {
            axes: [Axis::default(), Axis::default()],
            target: 0,
            monitor: 0,
            next: None,
            last_frame: None,
            epoch: 0,
        }
    }
    fn clear(&mut self) {
        self.axes.iter_mut().for_each(Axis::cancel);
        self.target = 0;
        self.next = None;
    }
    fn active(&self) -> bool {
        self.axes.iter().any(Axis::active)
    }
}

fn deliver(state: &Shared, motion: &mut Motion, delta: i32, horizontal: bool) -> bool {
    let mut remaining = delta;
    while remaining != 0 {
        // Windows wheel messages carry a signed 16-bit delta, even though
        // MOUSEINPUT.mouseData is 32-bit. A large overdue frame must be split.
        let piece = remaining.clamp(-32767, 32767);
        if !deliver_piece(state, motion, piece, horizontal) {
            return false;
        }
        remaining -= piece;
    }
    true
}

/// Decrements a positive counter; true when one unit was taken.
fn take_one(counter: &AtomicU32) -> bool {
    let mut current = counter.load(Ordering::Relaxed);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
    false
}

fn deliver_piece(state: &Shared, motion: &mut Motion, delta: i32, horizontal: bool) -> bool {
    if delta == 0 {
        return true;
    }
    if !state.enabled.load(Ordering::Acquire)
        || state.modifiers()
        || motion.epoch != state.cancel_epoch.load(Ordering::Relaxed)
        || (!state.verifying && pointer_root() != motion.target)
    {
        motion.clear();
        return false;
    }
    let simulate_failure = state.verifying && take_one(&state.test_fail_next);
    let input = wheel_input(
        delta,
        horizontal,
        if state.verifying {
            TEST_OUTPUT
        } else {
            OWN_MARKER
        },
    );
    let sent = if simulate_failure {
        0
    } else {
        unsafe {
            SetLastError(0);
            SendInput(1, &input, std::mem::size_of::<Input>() as i32)
        }
    };
    if sent == 1 {
        state.stats.emitted.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    let error = if simulate_failure {
        5
    } else {
        unsafe { GetLastError() }
    };
    state.stats.last_error.store(error, Ordering::Relaxed);
    state.stats.failures.fetch_add(1, Ordering::Relaxed);
    let root = motion.target;
    if let Ok(mut cache) = state.policies.write() {
        cache.insert(
            root,
            Policy {
                pid: if state.verifying { 0 } else { process_id(root) },
                allowed: false,
                fullscreen_document: false,
                line_scroller: false,
                temporary: true,
                expires: state.now() + if state.verifying { 250.0 } else { 2000.0 },
            },
        );
    }
    state.log(&format!(
        "wheel delivery rejected (Win32 {}); temporarily bypassing this window",
        error
    ));
    motion.clear();
    false
}

fn profile(config: &Config, lines_factor: u32) -> Profile {
    Profile {
        speed: config.speed * lines_factor,
        duration: config.duration,
        acceleration: config.acceleration,
    }
}

fn policy_of(state: &Shared, root: Handle) -> Option<Policy> {
    state
        .policies
        .read()
        .ok()
        .and_then(|cache| cache.get(&root).copied())
}

/// Applies the line-scroller adjustment only while one is the target.
fn follow_line_scroller(state: &Shared, config: &Config, root: Handle) {
    let wanted = !state.verifying
        && config.enabled
        && policy_of(state, root)
            .map(|p| p.allowed && p.line_scroller)
            .unwrap_or(false);
    if wanted {
        state.lines.enter();
    } else {
        state.lines.leave();
    }
}

// Chromium treats wheel deltas that are exact multiples of WHEEL_DELTA as
// coarse notches and animates them again with its own smooth scrolling;
// other values scroll precisely. Keep every animated frame precise.
fn precise(axis: &mut Axis, delta: i32) -> i32 {
    if delta != 0 && delta % 120 == 0 && axis.active() {
        axis.defer(delta.signum());
        delta - delta.signum()
    } else {
        delta
    }
}

fn worker_loop(
    state: Arc<Shared>,
    receiver: Receiver<Command>,
    mut config: Config,
    ready: mpsc::Sender<Result<(), String>>,
) {
    unsafe {
        SetThreadDpiAwarenessContext(-4);
    }
    set_thread_priority(2);
    let timer = unsafe { CreateWaitableTimerExW(null(), null(), 2, 0x001F0003) };
    if timer == 0 {
        let _ = ready.send(Err(format!(
            "High-resolution timer unavailable: {}",
            unsafe { GetLastError() }
        )));
        return;
    }
    if !state.verifying {
        let root = pointer_root();
        resolve_policy(&state, &config, root, process_id(root));
    }
    let _ = ready.send(Ok(()));
    let handles = [state.wake, timer];
    let mut motion = Motion::new();
    let mut frames = FrameClock::new(state.vsync.clone());
    let mut epoch = state.cancel_epoch.load(Ordering::Relaxed);
    'work: loop {
        let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if result == u32::MAX {
            state.log("worker wait failed; native input will pass through");
            state.enabled.store(false, Ordering::Release);
            break;
        }
        while state.verifying && state.test_hold_worker.load(Ordering::Relaxed) {
            thread::sleep(std::time::Duration::from_millis(1));
        }
        if state.modifiers() {
            state.cancel_epoch.fetch_add(1, Ordering::Relaxed);
        }
        let new_epoch = state.cancel_epoch.load(Ordering::Relaxed);
        if epoch != new_epoch || state.modifiers() || !state.enabled.load(Ordering::Acquire) {
            motion.clear();
            epoch = new_epoch;
        }
        let mut batch = 0;
        for _ in 0..256 {
            match receiver.try_recv() {
                Ok(Command::Stop) => break 'work,
                Ok(Command::Configure(new_config)) => {
                    config = new_config;
                    motion.clear();
                    if let Ok(mut cache) = state.policies.write() {
                        cache.clear();
                    }
                    if !state.verifying {
                        let root = pointer_root();
                        resolve_policy(&state, &config, root, process_id(root));
                        follow_line_scroller(&state, &config, root);
                    }
                }
                Ok(Command::Resolve(root, pid)) => resolve_policy(&state, &config, root, pid),
                Ok(Command::Hover(root)) => follow_line_scroller(&state, &config, root),
                Ok(Command::Wheel(event)) => {
                    if !state.enabled.load(Ordering::Acquire)
                        || state.modifiers()
                        || event.epoch != state.cancel_epoch.load(Ordering::Relaxed)
                    {
                        continue;
                    }
                    if !state.verifying && pointer_root() != event.root {
                        continue;
                    }
                    // A failed target stays in native pass-through mode for the cooldown.
                    if state
                        .policies
                        .read()
                        .ok()
                        .and_then(|p| p.get(&event.root).copied())
                        .map(|p| !p.allowed && state.now() < p.expires)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    if (motion.target != 0 && motion.target != event.root)
                        || motion.epoch != event.epoch
                    {
                        motion.clear();
                    }
                    if motion.target != event.root {
                        follow_line_scroller(&state, &config, event.root);
                    }
                    motion.target = event.root;
                    motion.epoch = event.epoch;
                    let delta = motion.axes[event.horizontal as usize].push(
                        event.delta,
                        profile(&config, state.lines.factor()),
                        event.at,
                    );
                    if delta != 0 {
                        deliver(&state, &mut motion, delta, event.horizontal);
                    }
                    if motion.active() && motion.next.is_none() {
                        motion.monitor = match state.test_monitor.load(Ordering::Relaxed) {
                            0 => monitor_of(motion.target),
                            monitor => monitor,
                        };
                        motion.next =
                            Some(frames.next(state.now(), motion.last_frame, motion.monitor));
                    }
                }
                Err(_) => break,
            }
            batch += 1;
        }
        // Drain bursts without allowing message traffic to starve frame delivery.
        let now = state.now();
        if let Some(scheduled) = motion.next.filter(|next| now >= *next) {
            if state.modifiers() || (!state.verifying && pointer_root() != motion.target) {
                motion.clear();
            } else {
                // Evaluate motion at the display slot, not at the wake-up time:
                // timer jitter must not become uneven distance between frames.
                let period = frames.period();
                let frame = scheduled + ((now - scheduled) / period).floor() * period;
                for axis in 0..2 {
                    let stepped = motion.axes[axis].step(frame);
                    let delta = precise(&mut motion.axes[axis], stepped);
                    if !deliver(&state, &mut motion, delta, axis == 1) {
                        break;
                    }
                }
                motion.last_frame = Some(frame);
                motion.next = if motion.active() {
                    Some(frames.next(state.now(), Some(frame), motion.monitor))
                } else {
                    None
                };
                state
                    .stats
                    .frame_period_us
                    .store((period * 1000.0) as u64, Ordering::Relaxed);
                state
                    .stats
                    .synchronized
                    .store(frames.synchronized(), Ordering::Relaxed);
            }
        }
        unsafe {
            if let Some(next) = motion.next {
                let due = -(((next - state.now()).max(0.05) * 10000.0) as i64);
                if SetWaitableTimer(timer, &due, 0, null(), null(), 0) == 0 {
                    state.log("timer scheduling failed; native input will pass through");
                    state.enabled.store(false, Ordering::Release);
                    motion.clear();
                }
            } else {
                CancelWaitableTimer(timer);
            }
        }
        // Auto-reset signals coalesce; explicitly drain the rest of a full batch.
        if batch == 256 {
            state.notify();
        }
    }
    unsafe {
        CancelWaitableTimer(timer);
        CloseHandle(timer);
    }
}
