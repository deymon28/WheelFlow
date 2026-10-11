//! Exercises real Win32 hooks and SendInput. An observer consumes both test input
//! and test output before either can scroll an application. No cursor/key changes.
use crate::{config::Config, runtime::Runtime, win::*};
use std::{
    ptr::null,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Observation {
    vertical: AtomicI64,
    horizontal: AtomicI64,
    raw: AtomicI64,
    pieces: AtomicU64,
    coarse: AtomicU64,
    thread: AtomicU32,
    measure_timing: AtomicBool,
    timing_index: AtomicU32,
    timing_origin: OnceLock<Instant>,
    timing_slots: OnceLock<Box<[AtomicU64]>>,
}
static OBSERVER: OnceLock<Arc<Observation>> = OnceLock::new();

unsafe extern "system" fn observe(code: i32, message: usize, data: isize) -> isize {
    if code >= 0 && (message == WHEEL || message == HWHEEL) && data != 0 {
        let event = &*(data as *const MouseHook);
        if event.extra == TEST_RAW || event.extra == TEST_OUTPUT {
            if let Some(observation) = OBSERVER.get() {
                let delta = (event.data >> 16) as u16 as i16 as i64;
                if event.extra == TEST_RAW {
                    observation.raw.fetch_add(delta, Ordering::Relaxed);
                } else {
                    if message == WHEEL {
                        observation.vertical.fetch_add(delta, Ordering::Relaxed);
                    } else {
                        observation.horizontal.fetch_add(delta, Ordering::Relaxed);
                    }
                    observation.pieces.fetch_add(1, Ordering::Relaxed);
                    if delta % 120 == 0 && observation.measure_timing.load(Ordering::Acquire) {
                        observation.coarse.fetch_add(1, Ordering::Relaxed);
                    }
                    if observation.measure_timing.load(Ordering::Acquire) {
                        let index = observation.timing_index.fetch_add(1, Ordering::Relaxed);
                        if let Some(slot) = observation
                            .timing_slots
                            .get()
                            .and_then(|s| s.get(index as usize))
                        {
                            slot.store(
                                observation
                                    .timing_origin
                                    .get()
                                    .unwrap()
                                    .elapsed()
                                    .as_micros() as u64,
                                Ordering::Release,
                            );
                        }
                    }
                }
            }
            return 1;
        }
    }
    CallNextHookEx(0, code, message, data)
}

fn wait_for(mut check: impl FnMut() -> bool, label: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !check() {
        if Instant::now() >= deadline {
            return Err(format!("Timed out: {}", label));
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn send(delta: i32, horizontal: bool) -> Result<(), String> {
    let input = wheel_input(delta, horizontal, TEST_RAW);
    let sent = unsafe { SendInput(1, &input, std::mem::size_of::<Input>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        Err(format!("Test SendInput rejected: {}", unsafe {
            GetLastError()
        }))
    }
}

pub fn run() -> Result<String, String> {
    let observed = Arc::new(Observation::default());
    let _ = observed.timing_origin.set(Instant::now());
    let _ = observed
        .timing_slots
        .set((0..512).map(|_| AtomicU64::new(0)).collect());
    OBSERVER
        .set(observed.clone())
        .map_err(|_| "Verifier already running")?;
    let observer_state = observed.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let observer_thread = thread::spawn(move || unsafe {
        let mut message = Msg::default();
        PeekMessageW(&mut message, 0, 0, 0, 0);
        observer_state
            .thread
            .store(GetCurrentThreadId(), Ordering::Relaxed);
        let hook = SetWindowsHookExW(14, Some(observe), GetModuleHandleW(null()), 0);
        let _ = ready_tx.send(hook != 0);
        if hook == 0 {
            return;
        }
        while GetMessageW(&mut message, 0, 0, 0) > 0 {}
        UnhookWindowsHookEx(hook);
    });
    if !ready_rx.recv().map_err(|e| e.to_string())? {
        return Err("Cannot install test observer; no input sent".into());
    }

    let result = exercise(&observed);
    unsafe {
        PostThreadMessageW(observed.thread.load(Ordering::Relaxed), WM_QUIT, 0, 0);
    }
    let _ = observer_thread.join();
    result
}

fn spin(observed: &Observation, notches: usize, spacing: Duration) -> Result<i64, String> {
    let before = observed.vertical.load(Ordering::Relaxed);
    for _ in 0..notches {
        send(120, false)?;
        thread::sleep(spacing);
    }
    let mut last = observed.vertical.load(Ordering::Relaxed);
    loop {
        thread::sleep(Duration::from_millis(150));
        let now = observed.vertical.load(Ordering::Relaxed);
        if now == last {
            return Ok(now - before);
        }
        last = now;
    }
}

fn frame_timing(
    observed: &Observation,
    state: &crate::runtime::Shared,
    monitor: Handle,
) -> Result<String, String> {
    observed.timing_index.store(0, Ordering::Relaxed);
    for slot in observed.timing_slots.get().unwrap().iter() {
        slot.store(0, Ordering::Relaxed);
    }
    let coarse = observed.coarse.load(Ordering::Relaxed);
    let before = observed.vertical.load(Ordering::Relaxed);
    observed.measure_timing.store(true, Ordering::Release);
    for _ in 0..15 {
        send(120, false)?;
        thread::sleep(Duration::from_millis(32));
    }
    let feed_end = observed.timing_origin.get().unwrap().elapsed().as_micros() as u64;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 1800,
        "overlapping 320ms impulses",
    )?;
    thread::sleep(Duration::from_millis(80));
    observed.measure_timing.store(false, Ordering::Release);
    let count = observed.timing_index.load(Ordering::Relaxed) as usize;
    let timestamps: Vec<_> = observed
        .timing_slots
        .get()
        .unwrap()
        .iter()
        .take(count.min(512))
        .map(|s| s.load(Ordering::Acquire))
        .filter(|t| *t > 0)
        .collect();
    if timestamps.len() < 30 {
        return Err("Smooth gesture did not produce enough independent output frames".into());
    }
    let mut steady: Vec<_> = timestamps
        .windows(2)
        .filter(|p| p[1] <= feed_end)
        .map(|p| p[1] - p[0])
        .collect();
    steady.sort_unstable();
    if steady.is_empty() {
        return Err("No timed outputs during continuous input".into());
    }
    let period_us = state.stats.frame_period_us.load(Ordering::Relaxed);
    if period_us == 0 {
        return Err("Output frame period was not measured".into());
    }
    let synchronized = state.stats.synchronized.load(Ordering::Relaxed);
    let nominal = monitor_device(monitor).as_ref().and_then(refresh_rate);
    // Timer wake-ups are quantized (typically 0.5 ms), so single intervals
    // alternate around the period; their mean shows whether frames were skipped.
    let mean = steady.iter().sum::<u64>() as f64 / steady.len() as f64;
    let mut report = format!(
        "  Display {} Hz; output {:.2} Hz ({})\n  Continuous-input output intervals: mean {:.3}ms, min {:.2}ms, median {:.2}ms, p95 {:.2}ms, max {:.2}ms\n",
        nominal.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
        1e6 / period_us as f64,
        if synchronized { "vertical-blank synchronized" } else { "fallback timer" },
        mean / 1000.0,
        steady[0] as f64 / 1000.0,
        steady[steady.len() / 2] as f64 / 1000.0,
        steady[(steady.len() - 1) * 95 / 100] as f64 / 1000.0,
        *steady.last().unwrap() as f64 / 1000.0
    );
    if !synchronized {
        return Err(format!(
            "{report}Output was not synchronized to this monitor"
        ));
    }
    if let Some(nominal) = nominal {
        let measured = 1e6 / period_us as f64;
        if (measured - nominal as f64).abs() > nominal as f64 * 0.02 {
            return Err(format!("{report}Output rate does not match this monitor"));
        }
    }
    if (mean - period_us as f64).abs() > period_us as f64 * 0.1 {
        return Err(format!(
            "{report}Continuous output does not follow the display frame period"
        ));
    }
    let doubled = steady
        .iter()
        .filter(|i| (**i as f64) < period_us as f64 * 0.5)
        .count();
    if doubled * 50 > steady.len() {
        return Err(format!(
            "{report}{doubled} outputs landed in the same display frame"
        ));
    }
    let coarse = observed.coarse.load(Ordering::Relaxed) - coarse;
    if coarse > 0 {
        return Err(format!(
            "{report}{coarse} animated outputs were multiples of 120"
        ));
    }
    report.push_str(&format!(
        "  PASS: one output per refresh; overlapping 320ms impulses preserve 1800 units over {} outputs, none a multiple of 120\n",
        count
    ));
    Ok(report)
}

fn exercise(observed: &Observation) -> Result<String, String> {
    let config = Config {
        speed: 100,
        duration: 100,
        enabled: true,
        acceleration: false,
        fullscreen_bypass: true,
        run_as_admin: false,
        exclusions: Vec::new(),
    };
    let mut runtime = Runtime::start(config.clone(), true)?;
    let state = runtime.shared.clone();
    let mut report = String::new();
    for delta in [1, -1, 15, -15, 60, -60, 120, -120, 960, -960] {
        let before = observed.vertical.load(Ordering::Relaxed);
        send(delta, false)?;
        wait_for(
            || observed.vertical.load(Ordering::Relaxed) == before + delta as i64,
            "wheel-distance delivery",
        )?;
        thread::sleep(Duration::from_millis(120));
        if observed.vertical.load(Ordering::Relaxed) != before + delta as i64 {
            return Err("Extra wheel output after settling".into());
        }
    }
    report.push_str(
        "PASS: signed wheel deltas 1, 15, 60, 120, 960 through native hook and SendInput\n",
    );

    let before = observed.vertical.load(Ordering::Relaxed);
    for _ in 0..500 {
        send(120, false)?;
    }
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 60000,
        "strong burst",
    )?;
    thread::sleep(Duration::from_millis(120));
    report.push_str("PASS: 500-event burst preserves 60000 wheel units\n");

    let before = observed.horizontal.load(Ordering::Relaxed);
    send(-120, true)?;
    wait_for(
        || observed.horizontal.load(Ordering::Relaxed) == before - 120,
        "horizontal delivery",
    )?;
    thread::sleep(Duration::from_millis(120));
    report.push_str("PASS: horizontal wheel delivery\n");

    let before = observed.vertical.load(Ordering::Relaxed);
    let sent_at = Instant::now();
    send(120, false)?;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) != before,
        "first output",
    )?;
    let latency = sent_at.elapsed();
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 120,
        "single notch",
    )?;
    thread::sleep(Duration::from_millis(120));
    if latency > Duration::from_millis(25) {
        return Err(format!("First output took {:?}", latency));
    }
    report.push_str(&format!(
        "PASS: first output {:.2}ms after a wheel notch\n",
        latency.as_secs_f64() * 1000.0
    ));

    let mut accelerated = config.clone();
    accelerated.acceleration = true;
    accelerated.duration = 250;
    state.update(accelerated);
    thread::sleep(Duration::from_millis(30));
    let slow = spin(observed, 6, Duration::from_millis(90))?;
    let fast = spin(observed, 20, Duration::from_millis(10))?;
    state.update(config.clone());
    thread::sleep(Duration::from_millis(30));
    if slow != 720 {
        return Err(format!("Slow notches were accelerated: {slow} of 720"));
    }
    if fast <= 2400 * 3 / 2 || fast > 2400 * 3 {
        return Err(format!("Fast spin distance {fast} outside 1.5x-3x of 2400"));
    }
    report.push_str(&format!(
        "PASS: acceleration keeps slow notches exact (720) and extends a fast spin to {fast} of 2400 units\n"
    ));

    let mut flowing = config.clone();
    flowing.duration = 320;
    state.update(flowing);
    thread::sleep(Duration::from_millis(30));
    let screens = monitors();
    for (index, monitor) in screens.iter().enumerate() {
        state.test_monitor.store(*monitor, Ordering::Relaxed);
        report.push_str(&format!("Monitor {} of {}:\n", index + 1, screens.len()));
        report.push_str(&frame_timing(observed, &state, *monitor)?);
    }
    state.test_monitor.store(0, Ordering::Relaxed);
    state.update(config.clone());
    thread::sleep(Duration::from_millis(30));

    let before = observed.vertical.load(Ordering::Relaxed);
    send(120, false)?;
    // Simulate a busy settings/UI thread beyond this machine's 200ms hook timeout.
    thread::sleep(Duration::from_millis(350));
    if observed.vertical.load(Ordering::Relaxed) != before + 120 {
        return Err("Output depends on UI message pumping".into());
    }
    send(120, false)?;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 240,
        "continued hook after UI stall",
    )?;
    thread::sleep(Duration::from_millis(120));
    report.push_str("PASS: output and hook survive 350ms UI-thread stall\n");

    let pieces = observed.pieces.load(Ordering::Relaxed);
    send(2400, false)?;
    wait_for(
        || observed.pieces.load(Ordering::Relaxed) > pieces,
        "start modifier cancellation case",
    )?;
    state.test_modifiers.store(true, Ordering::Relaxed);
    thread::sleep(Duration::from_millis(30));
    let stopped = observed.pieces.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(120));
    if observed.pieces.load(Ordering::Relaxed) != stopped {
        return Err("Pending output continued under modifier".into());
    }
    state.test_modifiers.store(false, Ordering::Relaxed);
    report.push_str("PASS: modifier state cancels pending animation\n");

    state.test_hold_worker.store(true, Ordering::Relaxed);
    let pieces = observed.pieces.load(Ordering::Relaxed);
    for _ in 0..500 {
        send(120, false)?;
    }
    state.cancel_pending();
    state.test_hold_worker.store(false, Ordering::Relaxed);
    thread::sleep(Duration::from_millis(250));
    if observed.pieces.load(Ordering::Relaxed) != pieces {
        return Err("Cancelled input backlog reconstructed motion".into());
    }
    report.push_str("PASS: cancellation rejects all 500 queued events\n");

    let mut fast = config.clone();
    fast.speed = 200;
    state.update(fast);
    thread::sleep(Duration::from_millis(30));
    state.test_hold_worker.store(true, Ordering::Relaxed);
    let before = observed.vertical.load(Ordering::Relaxed);
    for _ in 0..4 {
        send(32767, false)?;
    }
    thread::sleep(Duration::from_millis(150));
    state.test_hold_worker.store(false, Ordering::Relaxed);
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 262136,
        "large delayed frame without signed16 overflow",
    )?;
    thread::sleep(Duration::from_millis(120));
    let before = observed.vertical.load(Ordering::Relaxed);
    send(-32768, false)?;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before - 65536,
        "large negative frame",
    )?;
    thread::sleep(Duration::from_millis(120));
    state.update(config.clone());
    thread::sleep(Duration::from_millis(30));
    report.push_str(
        "PASS: delayed large frames preserve +262136 and -65536 units without signed16 wrap\n",
    );

    let failures = state.stats.failures.load(Ordering::Relaxed);
    state.test_fail_next.store(1, Ordering::Relaxed);
    send(120, false)?;
    wait_for(
        || state.stats.failures.load(Ordering::Relaxed) == failures + 1,
        "simulated delivery failure",
    )?;
    let raw = observed.raw.load(Ordering::Relaxed);
    send(120, false)?;
    wait_for(
        || observed.raw.load(Ordering::Relaxed) == raw + 120,
        "temporary target bypass",
    )?;
    thread::sleep(Duration::from_millis(300));
    let before = observed.vertical.load(Ordering::Relaxed);
    send(120, false)?;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 120,
        "recovery after delivery rejection",
    )?;
    if !state.enabled.load(Ordering::Relaxed) {
        return Err("Delivery failure changed user enabled preference".into());
    }
    thread::sleep(Duration::from_millis(120));
    report.push_str(
        "PASS: injected delivery failure only temporarily bypasses target, then recovers\n",
    );

    let mut changed = config;
    changed.enabled = false;
    state.update(changed.clone());
    let raw = observed.raw.load(Ordering::Relaxed);
    send(60, false)?;
    wait_for(
        || observed.raw.load(Ordering::Relaxed) == raw + 60,
        "disabled pass-through",
    )?;
    changed.enabled = true;
    let installed = state.stats.installed.load(Ordering::Relaxed);
    state.update(changed);
    wait_for(
        || state.stats.installed.load(Ordering::Relaxed) > installed,
        "reinstall hook",
    )?;
    thread::sleep(Duration::from_millis(30));
    let before = observed.vertical.load(Ordering::Relaxed);
    send(60, false)?;
    wait_for(
        || observed.vertical.load(Ordering::Relaxed) == before + 60,
        "re-enable",
    )?;
    report.push_str("PASS: disable, hook reinstall, re-enable\n");
    let max_us = state.stats.max_hook_us.load(Ordering::Relaxed);
    if max_us >= 200000 {
        return Err(format!(
            "Hook callback exceeded 200ms timeout: {}us",
            max_us
        ));
    }
    report.push_str(&format!(
        "Maximum callback work: {} us\nCaptured: {}\nEmitted pieces: {}\nQueue overflow: {}\n",
        max_us,
        state.stats.captured.load(Ordering::Relaxed),
        state.stats.emitted.load(Ordering::Relaxed),
        state.stats.queue_full.load(Ordering::Relaxed)
    ));
    runtime.stop();
    Ok(report)
}
