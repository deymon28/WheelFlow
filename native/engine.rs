//! Wheel units are integrated as floats and quantized only at the output.
//! Direction changes deliberately cancel unissued momentum.
use std::collections::VecDeque;

const MAX_IMPULSES: usize = 512;
// Glide decay: velocity falls to e^-DECAY (under 1%) of its peak at the end.
const DECAY: f64 = 5.0;
// Velocity reaches its peak within a few milliseconds regardless of the glide
// duration, so the first displayed frame already moves.
const ONSET_MS: f64 = 3.0;
// Same-direction input whose notch-equivalent spacing is shorter than this
// window accelerates. Spacing is normalized by delta, so high-resolution
// wheels that send many small deltas accelerate only when actually spun fast.
const ACCEL_WINDOW_MS: f64 = 60.0;
const ACCEL_MAX: f64 = 3.0;
const NOTCH: f64 = 120.0;

struct Impulse {
    distance: f64,
    start: f64,
    duration: f64,
    issued: f64,
}

// Velocity e^(-t/decay) - e^(-t/onset): a near-instant rise (a few ms, so no
// one-frame jump) followed by an exponential glide, like inertial scrolling.
// Integrated in closed form and normalized to end exactly at the duration.
// Overlapping impulses add linearly.
fn position(elapsed: f64, duration: f64) -> f64 {
    let elapsed = elapsed.clamp(0.0, duration);
    let decay = duration / DECAY;
    let onset = ONSET_MS.min(decay * 0.25);
    let integral = |t: f64| decay * (1.0 - (-t / decay).exp()) - onset * (1.0 - (-t / onset).exp());
    integral(elapsed) / integral(duration)
}

#[derive(Clone, Copy)]
pub struct Profile {
    pub speed: u32,
    pub duration: u32,
    pub acceleration: bool,
}

#[derive(Default)]
pub struct Axis {
    impulses: VecDeque<Impulse>,
    fraction: f64,
    direction: i32,
    last_input: Option<f64>,
    spacing: f64,
}

impl Axis {
    pub fn active(&self) -> bool {
        !self.impulses.is_empty()
    }

    fn acceleration(&mut self, delta: i32, now: f64, enabled: bool) -> f64 {
        let gap = self.last_input.map(|last| now - last);
        self.last_input = Some(now);
        match gap {
            Some(gap) if enabled && gap >= 0.0 && gap < ACCEL_WINDOW_MS => {
                let notches = (delta.unsigned_abs() as f64 / NOTCH).max(1.0 / NOTCH);
                let spacing = gap / notches;
                // Average spacing so a single quick pair cannot spike the factor.
                // Weight by wheel distance: fine wheels ramp up as fast as notches.
                let weight = 1.0 - 0.5f64.powf(notches.min(4.0));
                self.spacing += weight * (spacing - self.spacing);
                ((1.0 + ACCEL_WINDOW_MS / self.spacing.max(1.0)) / 2.0).clamp(1.0, ACCEL_MAX)
            }
            _ => {
                self.spacing = ACCEL_WINDOW_MS;
                1.0
            }
        }
    }

    pub fn push(&mut self, delta: i32, profile: Profile, now: f64) -> i32 {
        if delta == 0 {
            return 0;
        }
        if self.direction != 0 && self.direction != delta.signum() {
            self.cancel();
        }
        self.direction = delta.signum();
        let factor = self.acceleration(delta, now, profile.acceleration);
        let scaled = delta as f64 * profile.speed as f64 / 100.0 * factor;
        // A single output unit cannot be split any further; do not delay it.
        if !self.active() && scaled.abs() <= 1.0 {
            self.fraction += scaled;
            return self.take_whole();
        }
        if self.impulses.capacity() < MAX_IMPULSES {
            self.impulses.reserve(MAX_IMPULSES - self.impulses.len());
        }
        let mut distance = scaled;
        if self.impulses.len() == MAX_IMPULSES {
            // Only extreme input pressure merges the newest remaining impulse.
            // Preserve distance, bounded storage, and the final settling deadline.
            let last = self.impulses.pop_back().unwrap();
            distance += last.distance - last.issued;
        }
        self.impulses.push_back(Impulse {
            distance,
            start: now,
            duration: profile.duration.max(1) as f64,
            issued: 0.0,
        });
        0
    }

    pub fn step(&mut self, now: f64) -> i32 {
        self.impulses.retain_mut(|impulse| {
            let elapsed = now - impulse.start;
            let cumulative = impulse.distance * position(elapsed, impulse.duration);
            self.fraction += cumulative - impulse.issued;
            impulse.issued = cumulative;
            elapsed < impulse.duration
        });
        self.take_whole()
    }

    /// Returns unsent units to the residual, to be emitted with a later frame.
    pub fn defer(&mut self, units: i32) {
        self.fraction += units as f64;
    }

    fn take_whole(&mut self) -> i32 {
        // Correct floating point noise near an integer, without rounding each input.
        let nearest = self.fraction.round();
        if (nearest - self.fraction).abs() < 1e-8 {
            self.fraction = nearest;
        }
        // Round the running residual, not individual inputs. This avoids waiting
        // until the exact endpoint to emit the final one-unit tail.
        let whole = self.fraction.round() as i32;
        self.fraction -= whole as f64;
        whole
    }

    pub fn cancel(&mut self) {
        self.impulses.clear();
        self.fraction = 0.0;
        self.direction = 0;
        self.last_input = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{position, Axis, Profile};

    const FRAME: f64 = 1000.0 / 300.0;

    fn plain(speed: u32, duration: u32) -> Profile {
        Profile {
            speed,
            duration,
            acceleration: false,
        }
    }

    fn settle(axis: &mut Axis, start: f64) -> i32 {
        (1..=400).map(|i| axis.step(start + i as f64 * FRAME)).sum()
    }

    #[test]
    fn curve_has_exact_endpoints_and_is_monotonic() {
        for duration in [60.0, 250.0, 600.0] {
            assert_eq!(position(0.0, duration), 0.0);
            assert!((position(duration, duration) - 1.0).abs() < 1e-12);
            let mut previous = 0.0;
            for i in 1..=1000 {
                let p = position(duration * i as f64 / 1000.0, duration);
                assert!(p >= previous);
                previous = p;
            }
            // Responsive: most of the distance within the first half.
            assert!(position(duration * 0.5, duration) > 0.85);
        }
        // Moves on the first 300 Hz frame without jumping.
        let first = position(FRAME, 250.0);
        assert!(first > 0.01 && first < 0.05, "{first}");
        // A quarter of the distance is covered within 20 ms.
        assert!(position(20.0, 250.0) > 0.25);
    }

    #[test]
    fn all_wheel_sizes_and_signs_preserve_distance() {
        for delta in [1, -1, 15, -15, 60, -60, 120, -120, 960, -960, 32767] {
            let mut axis = Axis::default();
            let first = axis.push(delta, plain(100, 170), 0.0);
            assert_eq!(first + settle(&mut axis, 0.0), delta);
            assert!(!axis.active());
        }
    }

    #[test]
    fn fractional_speed_is_preserved_across_separate_gestures() {
        for speed in [50, 80, 125] {
            let mut axis = Axis::default();
            let mut total = 0;
            for i in 0..100 {
                let at = i as f64 * 2000.0;
                total += axis.push(1, plain(speed, 100), at);
                total += settle(&mut axis, at);
            }
            assert_eq!(total, speed as i32);
        }
    }

    #[test]
    fn strong_burst_has_no_queue_loss_or_unbounded_tail() {
        let mut axis = Axis::default();
        let mut total = 0;
        for i in 0..1000 {
            let now = i as f64;
            total += axis.push(120, plain(100, 170), now);
            if i % 3 == 0 {
                total += axis.step(now);
            }
        }
        total += settle(&mut axis, 1000.0);
        assert_eq!(total, 120000);
        assert!(!axis.active());
    }

    #[test]
    fn reverse_input_immediately_cancels_old_direction() {
        let mut axis = Axis::default();
        axis.push(2400, plain(100, 170), 0.0);
        assert!(axis.step(8.0) > 0);
        axis.push(-15, plain(100, 170), 10.0);
        let mut sum = 0;
        for i in 1..=100 {
            let value = axis.step(10.0 + i as f64 * FRAME);
            assert!(value <= 0);
            sum += value;
        }
        assert_eq!(sum, -15);
    }

    #[test]
    fn irregular_scheduling_finishes_without_a_final_large_backlog() {
        let mut axis = Axis::default();
        axis.push(-120, plain(100, 100), 0.0);
        let values: Vec<_> = [8.0, 25.0, 71.0, 220.0]
            .iter()
            .map(|t| axis.step(*t))
            .collect();
        assert_eq!(values.iter().sum::<i32>(), -120);
        assert!(values[3].abs() < 10);
        assert!(!axis.active());
    }

    #[test]
    fn cancellation_discards_future_output() {
        let mut axis = Axis::default();
        axis.push(120, plain(100, 100), 0.0);
        axis.cancel();
        assert_eq!(axis.step(300.0), 0);
    }

    #[test]
    fn single_notch_starts_immediately_without_a_jump_and_glides_out() {
        let mut axis = Axis::default();
        axis.push(120, plain(100, 250), 0.0);
        let frames: Vec<_> = (1..=80).map(|i| axis.step(i as f64 * FRAME)).collect();
        assert_eq!(frames.iter().sum::<i32>(), 120);
        assert!(frames[0] >= 1, "the first frame already moves");
        let peak = frames.iter().copied().max().unwrap();
        assert!(peak <= 9, "no frame carries a large step: {peak}");
        let peak_at = frames.iter().position(|v| *v == peak).unwrap();
        assert!(peak_at <= 5, "peak speed within 20 ms: frame {peak_at}");
        assert!(frames[..3].iter().sum::<i32>() >= 12, "10% within 10 ms");
        assert!(frames[..36].iter().sum::<i32>() >= 108, "90% within 120 ms");
        assert!(frames[60..].iter().sum::<i32>() <= 2, "tail fades out");
        // Successive frames never speed up after the peak: no lurches.
        for pair in frames[peak_at + 1..].windows(2) {
            assert!(pair[1] <= pair[0] + 1, "{frames:?}");
        }
        assert!(!axis.active());
    }

    #[test]
    fn deferred_units_are_emitted_later_without_loss() {
        let mut axis = Axis::default();
        axis.push(1200, plain(100, 250), 0.0);
        let mut total = 0;
        for i in 1..=100 {
            let mut value = axis.step(i as f64 * FRAME);
            if value != 0 && value % 120 == 0 && axis.active() {
                value -= value.signum();
                axis.defer(value.signum());
            }
            total += value;
        }
        assert_eq!(total, 1200);
    }

    #[test]
    fn new_notch_does_not_restart_or_speed_up_an_existing_impulse() {
        let mut combined = Axis::default();
        let mut first = Axis::default();
        let mut second = Axis::default();
        let profile = plain(100, 320);
        combined.push(12000, profile, 0.0);
        first.push(12000, profile, 0.0);
        for at in [8.0, 40.0, 80.0] {
            assert_eq!(combined.step(at), first.step(at));
        }
        combined.push(12000, profile, 80.0);
        second.push(12000, profile, 80.0);
        let mut difference: i32 = 0;
        for i in 11..=50 {
            let at = i as f64 * 8.0;
            difference += combined.step(at) - first.step(at) - second.step(at);
            assert!(difference.abs() <= 1, "overlap must be a linear sum");
        }
        assert_eq!(difference, 0);
        assert!(!combined.active());
    }

    #[test]
    fn extreme_input_pressure_is_bounded_and_preserves_remaining_distance() {
        let mut axis = Axis::default();
        let mut total = 0;
        for i in 0..10000 {
            total += axis.push(120, plain(80, 320), i as f64 / 100.0);
        }
        assert!(axis.impulses.len() <= super::MAX_IMPULSES);
        total += axis.step(500.0);
        assert_eq!(total, 960000);
        assert!(!axis.active());
    }

    fn spin(interval: f64, notches: usize, delta: i32) -> i32 {
        let mut axis = Axis::default();
        let profile = Profile {
            speed: 100,
            duration: 250,
            acceleration: true,
        };
        let mut total = 0;
        for i in 0..notches {
            total += axis.push(delta, profile, i as f64 * interval);
        }
        total + settle(&mut axis, notches as f64 * interval)
    }

    #[test]
    fn slow_and_isolated_notches_are_not_accelerated() {
        assert_eq!(spin(100.0, 10, 120), 1200);
        assert_eq!(spin(60.0, 10, -120), -1200);
        // A high-resolution wheel sending 15-unit steps every 10 ms is slow.
        assert_eq!(spin(10.0, 80, 15), 1200);
    }

    #[test]
    fn fast_spins_accelerate_gradually_and_bounded() {
        let moderate = spin(30.0, 10, 120);
        let fast = spin(12.0, 10, 120);
        let extreme = spin(1.0, 40, 120);
        assert!(moderate > 1200 && moderate < 1800, "{moderate}");
        assert!(fast > moderate && fast < 3000, "{fast}");
        assert!(extreme <= 40 * 360, "{extreme}");
        // The first notch of a gesture always travels its normal distance.
        let mut axis = Axis::default();
        let profile = Profile {
            speed: 100,
            duration: 250,
            acceleration: true,
        };
        axis.push(120, profile, 0.0);
        assert_eq!(settle(&mut axis, 0.0), 120);
        // Equivalent high-resolution input accelerates like notches.
        let fine = spin(12.0 / 8.0, 80, 15);
        assert!((fine - fast).abs() < fast / 10, "{fine} vs {fast}");
    }

    #[test]
    fn reversal_resets_acceleration() {
        let mut axis = Axis::default();
        let profile = Profile {
            speed: 100,
            duration: 250,
            acceleration: true,
        };
        for i in 0..10 {
            axis.push(120, profile, i as f64 * 10.0);
        }
        axis.push(-120, profile, 100.0);
        assert_eq!(settle(&mut axis, 100.0), -120);
    }
}
