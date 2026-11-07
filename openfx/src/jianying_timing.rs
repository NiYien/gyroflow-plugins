// SPDX-License-Identifier: GPL-3.0-or-later
use ofx::{Error, Result};

/// The source clip's OFX clock, shared by both Jianying platform adapters.
/// Requests can be fractional and can already include host trim or retiming.
/// Clip range endpoints describe image availability, not a playback-speed map.
/// Neither project fps nor output frame count identifies the delivered image.
pub(super) struct SourceClock {
    fps: f64,
}

fn checked_timestamp(timestamp: f64) -> Result<i64> {
    let rounded = timestamp.round();
    if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded >= i64::MAX as f64 {
        return Err(Error::InvalidValue);
    }
    Ok(rounded as i64)
}

impl SourceClock {
    pub(super) fn new(fps: f64) -> Result<Self> {
        if !fps.is_finite() || fps <= 0.0 { return Err(Error::InvalidValue); }
        Ok(Self { fps })
    }

    pub(super) fn timestamp_us(&self, frame: f64) -> Result<i64> {
        checked_timestamp(frame / self.fps * 1_000_000.0)
    }

    pub(super) fn frame_at_timestamp_us(&self, timestamp: i64) -> Result<f64> {
        let frame = timestamp as f64 / 1_000_000.0 * self.fps;
        if !frame.is_finite() { return Err(Error::InvalidValue); }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_mac24_clock_is_preserved() {
        let clock = SourceClock::new(30.0).unwrap();
        for (frame, timestamp) in [(0.0, 0), (7.0, 233_333), (14.0, 466_667),
            (15.0, 500_000), (21.0, 700_000), (28.0, 933_333), (29.0, 966_667)] {
            assert_eq!(clock.timestamp_us(frame).unwrap(), timestamp);
        }
    }

    #[test]
    fn captured_windows_source_times_do_not_floor_to_project_frames() {
        let clock = SourceClock::new(30.0).unwrap();
        // These request times were independently matched to source images.
        for (frame, timestamp) in [(30.53049, 1_017_683), (60.56049, 2_018_683),
            (90.59049, 3_019_683), (120.62049, 4_020_683)] {
            assert_eq!(clock.timestamp_us(frame).unwrap(), timestamp);
        }
        // A fractional request is not proof of floor or nearest source sampling.
        assert_eq!(clock.timestamp_us(30.49).unwrap(), 1_016_333);
        assert_eq!(clock.timestamp_us(30.5).unwrap(), 1_016_667);
    }

    #[test]
    fn source_clock_keeps_trim_reverse_freeze_and_retime_requests() {
        let clock = SourceClock::new(30.0).unwrap();
        // The host supplies source positions; no global rate is inferred here.
        for (frame, timestamp) in [(60.0, 2_000_000), (75.0, 2_500_000),
            (90.0, 3_000_000), (90.0, 3_000_000), (45.0, 1_500_000), (-15.0, -500_000)] {
            assert_eq!(clock.timestamp_us(frame).unwrap(), timestamp);
        }
    }

    #[test]
    fn project_ramp_image_request_round_trips_without_frame_rounding() {
        for fps in [24.0, 30.0, 60_000.0 / 1001.0] {
            let clock = SourceClock::new(fps).unwrap();
            for timestamp in [0, 18_333, 233_333, 1_017_683, 4_020_683, -16_667] {
                let frame = clock.frame_at_timestamp_us(timestamp).unwrap();
                assert_eq!(clock.timestamp_us(frame).unwrap(), timestamp);
            }
        }
    }

    #[test]
    fn invalid_clock_and_overflow_are_rejected() {
        for fps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(SourceClock::new(fps).is_err());
        }
        let clock = SourceClock::new(30.0).unwrap();
        for frame in [f64::NAN, f64::INFINITY, f64::MAX, i64::MAX as f64] {
            assert!(clock.timestamp_us(frame).is_err());
        }
        assert!(SourceClock::new(f64::MAX).unwrap().frame_at_timestamp_us(i64::MAX).is_err());
    }
}
