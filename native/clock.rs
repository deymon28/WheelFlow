//! Display-synchronized output frames. One output per refresh of the monitor
//! showing the target keeps every rendered frame advancing by an equal step;
//! a cadence from another monitor or a fixed timer produces alternating
//! 0/1/2 outputs per displayed frame, which is visible as judder.
use crate::win::*;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};

const FALLBACK_PERIOD_MS: f64 = 1000.0 / 60.0;
// Keep observing vertical blanks briefly after motion ends, so the next
// gesture starts on a known phase.
const LINGER_MS: f64 = 2000.0;
// Send shortly after a vertical blank, leaving most of the frame for the
// target application to handle the message and render. Waitable timers fire
// late rather than early, so the margin absorbs typical wake-up jitter.
const PHASE: f64 = 0.25;

/// Vertical-blank observations for one monitor, published by a dedicated thread.
pub struct Vsync {
    origin: Instant,
    wake: Handle,
    requested: AtomicIsize,
    tracked: AtomicIsize,
    last_us: AtomicU64,
    period_ns: AtomicU64,
    active_until_us: AtomicU64,
    stop: AtomicBool,
}

impl Vsync {
    pub fn start(origin: Instant) -> Arc<Self> {
        let vsync = Arc::new(Self {
            origin,
            // Never closed: the thread may still be blocked in a vertical-blank
            // wait at shutdown, and the process exits right after.
            wake: unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) },
            requested: AtomicIsize::new(0),
            tracked: AtomicIsize::new(0),
            last_us: AtomicU64::new(0),
            period_ns: AtomicU64::new(0),
            active_until_us: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let worker = vsync.clone();
        if vsync.wake == 0
            || thread::Builder::new()
                .name("vertical-blank".into())
                .spawn(move || worker.run())
                .is_err()
        {
            vsync.stop.store(true, Ordering::Relaxed);
        }
        vsync
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe {
            SetEvent(self.wake);
        }
    }

    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }

    /// Requests observation of `monitor` for the next couple of seconds.
    fn track(&self, monitor: Handle) {
        let now = self.now_us();
        let was_idle = self.active_until_us.load(Ordering::Relaxed) <= now;
        self.active_until_us
            .store(now + (LINGER_MS * 1000.0) as u64, Ordering::Relaxed);
        if self.requested.swap(monitor, Ordering::Relaxed) != monitor || was_idle {
            unsafe {
                SetEvent(self.wake);
            }
        }
    }

    /// The latest vertical blank (ms) and refresh period (ms) for `monitor`, if fresh.
    fn latest(&self, monitor: Handle, now: f64) -> Option<(f64, f64)> {
        if self.tracked.load(Ordering::Acquire) != monitor {
            return None;
        }
        let last = self.last_us.load(Ordering::Acquire) as f64 / 1000.0;
        let period = self.period_ns.load(Ordering::Acquire) as f64 / 1e6;
        (period > 0.0 && now - last < period * 4.0).then_some((last, period))
    }

    fn run(self: Arc<Self>) {
        set_thread_priority(15);
        let mut adapter: Option<(Handle, OpenAdapter)> = None;
        let mut period = 0.0f64;
        let mut previous: Option<f64> = None;
        let mut suspicious = 0;
        while !self.stop.load(Ordering::Relaxed) {
            let now = self.now_us();
            let until = self.active_until_us.load(Ordering::Relaxed);
            if until <= now {
                self.tracked.store(0, Ordering::Release);
                previous = None;
                unsafe {
                    WaitForSingleObject(self.wake, INFINITE);
                }
                continue;
            }
            let monitor = self.requested.load(Ordering::Relaxed);
            if adapter.as_ref().map(|(m, _)| *m) != Some(monitor) {
                if let Some((_, open)) = adapter.take() {
                    unsafe {
                        D3DKMTCloseAdapter(&open.adapter);
                    }
                }
                self.tracked.store(0, Ordering::Release);
                previous = None;
                let device = monitor_device(monitor);
                period = device
                    .as_ref()
                    .and_then(refresh_rate)
                    .map(|hz| 1000.0 / hz as f64)
                    .unwrap_or(FALLBACK_PERIOD_MS);
                adapter = device.and_then(|device| {
                    let mut open = OpenAdapter {
                        device,
                        adapter: 0,
                        luid_low: 0,
                        luid_high: 0,
                        source: 0,
                    };
                    (unsafe { D3DKMTOpenAdapterFromGdiDisplayName(&mut open) } == 0)
                        .then_some((monitor, open))
                });
                if adapter.is_none() {
                    // No vertical-blank source: the worker uses its timer fallback.
                    unsafe {
                        WaitForSingleObject(self.wake, 1000);
                    }
                    continue;
                }
            }
            let (_, open) = adapter.as_ref().unwrap();
            let wait = WaitForVerticalBlank {
                adapter: open.adapter,
                device: 0,
                source: open.source,
            };
            let status = unsafe { D3DKMTWaitForVerticalBlankEvent(&wait) };
            let at = self.origin.elapsed().as_secs_f64() * 1000.0;
            let interval = previous.map(|p| at - p);
            previous = Some(at);
            // Some drivers return immediately while a display is off; never
            // publish or spin on such results.
            if status != 0 || interval.map(|i| i < period * 0.3).unwrap_or(false) {
                suspicious += 1;
                if status != 0 || suspicious > 3 {
                    self.tracked.store(0, Ordering::Release);
                    if let Some((_, open)) = adapter.take() {
                        unsafe {
                            D3DKMTCloseAdapter(&open.adapter);
                        }
                    }
                    unsafe {
                        WaitForSingleObject(self.wake, 250);
                    }
                    suspicious = 0;
                }
                continue;
            }
            suspicious = 0;
            if let Some(interval) = interval {
                if (period * 0.7..period * 1.3).contains(&interval) {
                    period += 0.05 * (interval - period);
                }
            }
            self.period_ns
                .store((period * 1e6) as u64, Ordering::Release);
            self.last_us.store((at * 1000.0) as u64, Ordering::Release);
            self.tracked.store(monitor, Ordering::Release);
        }
        if let Some((_, open)) = adapter.take() {
            unsafe {
                D3DKMTCloseAdapter(&open.adapter);
            }
        }
    }
}

pub struct FrameClock {
    vsync: Arc<Vsync>,
    monitor: Handle,
    period: f64,
    anchor: f64,
    synchronized: bool,
}

impl FrameClock {
    pub fn new(vsync: Arc<Vsync>) -> Self {
        Self {
            vsync,
            monitor: 0,
            period: FALLBACK_PERIOD_MS,
            anchor: 0.0,
            synchronized: false,
        }
    }

    pub fn period(&self) -> f64 {
        self.period
    }

    pub fn synchronized(&self) -> bool {
        self.synchronized
    }

    /// The first output slot after `now` for a target shown on `monitor`,
    /// at least half a period after the `previous` output.
    pub fn next(&mut self, now: f64, previous: Option<f64>, monitor: Handle) -> f64 {
        self.vsync.track(monitor);
        if let Some((vblank, period)) = self.vsync.latest(monitor, now) {
            self.period = period;
            self.anchor = vblank + PHASE * period;
            self.synchronized = true;
        } else if monitor != self.monitor || self.synchronized {
            // Until the first vertical blank is observed, use the monitor's
            // nominal rate and start immediately rather than waiting for it.
            self.period = monitor_device(monitor)
                .as_ref()
                .and_then(refresh_rate)
                .map(|hz| 1000.0 / hz as f64)
                .unwrap_or(FALLBACK_PERIOD_MS);
            self.anchor = now - self.period + 0.25;
            self.synchronized = false;
        }
        self.monitor = monitor;
        next_slot(self.anchor, self.period, now, previous)
    }
}

fn next_slot(anchor: f64, period: f64, now: f64, previous: Option<f64>) -> f64 {
    let after = previous.map(|p| now.max(p + period * 0.5)).unwrap_or(now);
    // Skip missed slots instead of sending catch-up frames back to back.
    anchor + (((after - anchor) / period).floor() + 1.0) * period
}

#[cfg(test)]
mod tests {
    use super::next_slot;

    #[test]
    fn slots_follow_the_refresh_grid_without_drift_or_double_frames() {
        for period in [1000.0 / 300.0, 10.0, 1000.0 / 60.0] {
            let anchor = 0.5;
            let mut slot = next_slot(anchor, period, 0.0, None);
            assert!(slot > 0.0 && slot <= period + anchor);
            for _ in 0..3000 {
                // Simulate up to 1 ms of late wake-up and work each frame.
                let next = next_slot(anchor, period, slot + 0.9, Some(slot));
                assert!((next - slot - period).abs() < 1e-6);
                slot = next;
            }
            let cycles = (slot - anchor) / period;
            assert!((cycles - cycles.round()).abs() < 1e-6);
            // A long stall skips slots rather than bunching them.
            let late = next_slot(anchor, period, slot + 10.0 * period, Some(slot));
            assert!(late > slot + 10.0 * period);
            // A phase correction never produces two outputs within one frame.
            let shifted = next_slot(anchor + period * 0.9, period, slot, Some(slot));
            assert!(shifted - slot >= period * 0.5);
        }
    }
}
