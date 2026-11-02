//! Sample timeline shared by a capture stream and the recording attached to it.
//!
//! Each chunk read from parec re-anchors the sample count to the monotonic
//! clock at its arrival. A command's arrival instant therefore maps to a
//! sample index through the newest chunk only, so clock drift and dropped
//! samples earlier in a long-running stream never shift the mapping.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Audio kept from before the start command.
pub const PREROLL: Duration = Duration::from_millis(450);
/// Audio kept after the stop command.
pub const TAIL: Duration = Duration::from_millis(200);
/// History retained by an idle warm stream.
pub const RING: Duration = Duration::from_secs(10);
/// Recordings whose start-to-stop span is shorter than this are discarded.
pub const MIN_SPEECH: Duration = Duration::from_millis(300);
/// Recordings whose loudest window stays below this level are discarded.
pub const SILENCE_DBFS: f64 = -50.0;
/// Window length for the loudness gate.
pub const GATE_WINDOW: Duration = Duration::from_millis(30);
/// Extrapolation past the newest chunk covers samples still in flight, but a
/// stalled stream must not place a start command far beyond the received
/// audio, or the recording would skip what arrives when delivery resumes.
const MAX_LEAD: Duration = Duration::from_millis(100);
/// Running window sums are recomputed this often to bound rounding drift.
const RESYNC_SAMPLES: usize = 4096;

pub fn samples_for(duration: Duration, sample_rate: u32) -> u64 {
    let samples = duration.as_nanos() * u128::from(sample_rate) / 1_000_000_000;
    u64::try_from(samples).unwrap_or(u64::MAX)
}

pub fn duration_for(samples: usize, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        return Duration::ZERO;
    }
    let nanos = samples as u128 * 1_000_000_000 / u128::from(sample_rate);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

pub(crate) struct Timeline {
    sample_rate: u32,
    ring: VecDeque<f32>,
    ring_capacity: usize,
    /// Index of the next sample to arrive.
    total: u64,
    /// Arrival instant of the newest chunk and `total` after it.
    anchor: Option<(Instant, u64)>,
    session: Option<SessionCapture>,
    ended: bool,
    error: Option<String>,
}

struct SessionCapture {
    /// Absolute index of `buffer[0]`.
    start: u64,
    speech_start: u64,
    speech_end: Option<u64>,
    /// Exclusive absolute end; unset while the recording is running.
    end: Option<u64>,
    buffer: Vec<f32>,
    max_samples: usize,
}

/// Audio taken from a finished session.
#[derive(Debug, Default)]
pub(crate) struct Captured {
    pub samples: Vec<f32>,
    /// Samples before the start command.
    pub preroll: usize,
    /// Offset of the stop command in `samples`; later samples are the tail.
    pub speech_end: usize,
}

impl Captured {
    pub fn tail(&self) -> usize {
        self.samples.len().saturating_sub(self.speech_end)
    }
}

impl SessionCapture {
    fn append(&mut self, first: u64, samples: &[f32]) {
        let len = samples.len() as u64;
        let from = self.start.saturating_sub(first).min(len);
        let to = self
            .end
            .map_or(len, |end| end.saturating_sub(first).min(len));
        if from >= to {
            return;
        }
        let slice = &samples[from as usize..to as usize];
        let room = self.max_samples.saturating_sub(self.buffer.len());
        let keep = &slice[..slice.len().min(room)];
        if keep.is_empty() || self.buffer.try_reserve(keep.len()).is_err() {
            return;
        }
        self.buffer.extend_from_slice(keep);
    }

    fn is_full(&self) -> bool {
        self.buffer.len() >= self.max_samples
    }
}

impl Timeline {
    pub fn new(sample_rate: u32, ring_capacity: usize) -> Self {
        Self {
            sample_rate,
            ring: VecDeque::with_capacity(ring_capacity),
            ring_capacity,
            total: 0,
            anchor: None,
            session: None,
            ended: false,
            error: None,
        }
    }

    pub fn push(&mut self, samples: &[f32], arrived: Instant) {
        if let Some(session) = &mut self.session {
            session.append(self.total, samples);
        }
        let keep = &samples[samples.len().saturating_sub(self.ring_capacity)..];
        let overflow = (self.ring.len() + keep.len()).saturating_sub(self.ring_capacity);
        self.ring.drain(..overflow);
        self.ring.extend(keep.iter().copied());
        self.total += samples.len() as u64;
        self.anchor = Some((arrived, self.total));
    }

    #[cfg(test)]
    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn last_chunk_at(&self) -> Option<Instant> {
        self.anchor.map(|(arrived, _)| arrived)
    }

    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn mark_ended(&mut self, error: Option<String>) {
        self.ended = true;
        if self.error.is_none() {
            self.error = error;
        }
    }

    fn oldest(&self) -> u64 {
        self.total - self.ring.len() as u64
    }

    fn max_lead(&self) -> u64 {
        samples_for(MAX_LEAD, self.sample_rate)
    }

    /// Maps a monotonic instant to a sample index through the newest chunk.
    fn index_at(&self, at: Instant) -> Option<u64> {
        let (arrived, index) = self.anchor?;
        Some(if at >= arrived {
            index.saturating_add(samples_for(at - arrived, self.sample_rate))
        } else {
            index.saturating_sub(samples_for(arrived - at, self.sample_rate))
        })
    }

    /// Attaches a recording whose start command arrived at `at`, keeping up
    /// to `preroll` buffered samples before it. Returns the samples kept.
    pub fn begin_session(&mut self, at: Instant, preroll: u64, max_samples: usize) -> usize {
        let speech_start = self
            .index_at(at)
            .map_or(self.total, |index| index.min(self.total + self.max_lead()));
        let start = speech_start.saturating_sub(preroll).max(self.oldest());
        // A command older than the ring keeps all of it, none as pre-roll.
        let speech_start = speech_start.max(start);
        self.attach(start, speech_start, max_samples);
        (speech_start - start) as usize
    }

    /// Attaches a recording to a stream opened for it: every sample the
    /// stream delivered or will deliver belongs to the recording.
    pub fn begin_session_at_stream_start(&mut self, max_samples: usize) {
        let start = self.oldest();
        self.attach(start, start, max_samples);
    }

    fn attach(&mut self, start: u64, speech_start: u64, max_samples: usize) {
        let mut buffer = Vec::new();
        if start < self.total {
            let skip = (start - self.oldest()) as usize;
            let keep = (self.ring.len() - skip).min(max_samples);
            buffer.reserve_exact(keep);
            buffer.extend(self.ring.range(skip..skip + keep).copied());
        }
        self.session = Some(SessionCapture {
            start,
            speech_start,
            speech_end: None,
            end: None,
            buffer,
            max_samples,
        });
    }

    /// Marks the stop command at `at` and returns the exclusive sample index
    /// the session needs before it can be finalized. Unlike the start, the
    /// stop is not capped at the received audio: during a delivery pause the
    /// caller waits (bounded) for the buffered audio instead of cutting the
    /// last word short.
    pub fn end_session(&mut self, at: Instant, tail: u64) -> Option<u64> {
        let speech_end = self.index_at(at).unwrap_or(self.total);
        let session = self.session.as_mut()?;
        let speech_end = speech_end.max(session.speech_start);
        let end = speech_end.saturating_add(tail);
        session.speech_end = Some(speech_end);
        session.end = Some(end);
        let len = end.saturating_sub(session.start);
        if (session.buffer.len() as u64) > len {
            session.buffer.truncate(len as usize);
        }
        Some(end)
    }

    /// True once the session has every sample up to `end`, or can get no more.
    pub fn reached(&self, end: u64) -> bool {
        self.total >= end || self.ended || self.session.as_ref().is_none_or(SessionCapture::is_full)
    }

    #[cfg(test)]
    pub fn session_capacity(&self) -> usize {
        self.session
            .as_ref()
            .map_or(0, |session| session.buffer.capacity())
    }

    pub fn take_session(&mut self) -> Option<Captured> {
        let session = self.session.take()?;
        let len = session.buffer.len();
        let offset = |index: u64| index.saturating_sub(session.start).min(len as u64) as usize;
        let preroll = offset(session.speech_start);
        let speech_end = session.speech_end.map_or(len, offset);
        Some(Captured {
            samples: session.buffer,
            preroll,
            speech_end,
        })
    }
}

/// Why a finished recording was not sent for transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    TooShort,
    Silent,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Keep => "keep",
            Verdict::TooShort => "too short",
            Verdict::Silent => "no speech",
        }
    }
}

pub fn gate(speech: Duration, loudest_dbfs: f64) -> Verdict {
    if speech < MIN_SPEECH {
        Verdict::TooShort
    } else if loudest_dbfs < SILENCE_DBFS {
        Verdict::Silent
    } else {
        Verdict::Keep
    }
}

/// Level of the loudest `window`-sample span, after removing each span's mean,
/// in dB relative to a full-scale amplitude of 1.0. Input shorter than a
/// window is measured as one span; empty or constant input is `-inf`.
pub fn loudest_window_dbfs(samples: &[f32], window: usize) -> f64 {
    let window = window.max(1).min(samples.len());
    if window == 0 {
        return f64::NEG_INFINITY;
    }
    let sums = |span: &[f32]| {
        span.iter().fold((0.0f64, 0.0f64), |(sum, sum_sq), &x| {
            let x = f64::from(x);
            (sum + x, sum_sq + x * x)
        })
    };
    let n = window as f64;
    let variance = |sum: f64, sum_sq: f64| (sum_sq / n - (sum / n).powi(2)).max(0.0);

    let (mut sum, mut sum_sq) = sums(&samples[..window]);
    let mut loudest = variance(sum, sum_sq);
    for start in 1..=samples.len() - window {
        if start % RESYNC_SAMPLES == 0 {
            (sum, sum_sq) = sums(&samples[start..start + window]);
        } else {
            let gone = f64::from(samples[start - 1]);
            let added = f64::from(samples[start + window - 1]);
            sum += added - gone;
            sum_sq += added * added - gone * gone;
        }
        loudest = loudest.max(variance(sum, sum_sq));
    }
    if loudest > 0.0 {
        10.0 * loudest.log10()
    } else {
        f64::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    /// Pushes `count` samples whose value is their own index, in 10 ms chunks
    /// arriving on a perfect clock starting at `epoch`.
    fn feed(timeline: &mut Timeline, epoch: Instant, count: usize) {
        for _ in 0..count / 160 {
            let first = timeline.total as usize;
            let chunk: Vec<f32> = (first..first + 160).map(|index| index as f32).collect();
            let arrived = epoch + duration_for(first + 160, RATE);
            timeline.push(&chunk, arrived);
        }
    }

    #[test]
    fn ring_keeps_only_the_newest_capacity() {
        let mut timeline = Timeline::new(RATE, 4);
        let now = Instant::now();
        timeline.push(&[1.0, 2.0, 3.0], now);
        timeline.push(&[4.0, 5.0, 6.0], now);
        assert_eq!(
            timeline.ring.iter().copied().collect::<Vec<_>>(),
            [3.0, 4.0, 5.0, 6.0]
        );
        assert_eq!(timeline.oldest(), 2);

        timeline.push(&[7.0, 8.0, 9.0, 10.0, 11.0, 12.0], now);
        assert_eq!(
            timeline.ring.iter().copied().collect::<Vec<_>>(),
            [9.0, 10.0, 11.0, 12.0]
        );
        assert_eq!(timeline.total, 12);
        assert_eq!(timeline.oldest(), 8);
    }

    #[test]
    fn preroll_and_tail_map_command_instants_to_sample_indices() {
        let mut timeline = Timeline::new(RATE, samples_for(RING, RATE) as usize);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 32_000);

        // The start command arrived exactly at sample 24_000 (1.5 s).
        let start_at = epoch + ms(1500);
        let preroll = timeline.begin_session(start_at, samples_for(PREROLL, RATE), usize::MAX);
        assert_eq!(preroll, 7_200);

        feed(&mut timeline, epoch, 16_000);
        // The stop command arrived at 2.5 s; the tail needs 3_200 more samples.
        let end = timeline.end_session(epoch + ms(2500), samples_for(TAIL, RATE));
        assert_eq!(end, Some(40_000 + 3_200));
        assert!(timeline.reached(43_200));

        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.samples.first(), Some(&16_800.0));
        assert_eq!(captured.samples.last(), Some(&43_199.0));
        assert_eq!(captured.preroll, 7_200);
        assert_eq!(captured.speech_end, 7_200 + 16_000);
        assert_eq!(captured.tail(), 3_200);
        assert!(
            captured
                .samples
                .windows(2)
                .all(|pair| pair[1] == pair[0] + 1.0)
        );
    }

    #[test]
    fn command_before_newest_chunk_maps_backwards() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 16_000);
        // The handler runs late; the command itself arrived at 0.8 s.
        timeline.begin_session(epoch + ms(800), 0, usize::MAX);
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.samples.first(), Some(&12_800.0));
        assert_eq!(captured.preroll, 0);
    }

    #[test]
    fn preroll_is_limited_to_buffered_history() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 1_600);
        let preroll = timeline.begin_session(epoch + ms(100), 7_200, usize::MAX);
        assert_eq!(preroll, 1_600);
    }

    #[test]
    fn stalled_stream_does_not_skip_audio_after_it_resumes() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 1_600);
        // No chunk for two seconds; the start command must not land 2 s ahead.
        timeline.begin_session(epoch + Duration::from_secs(2), 7_200, usize::MAX);
        assert_eq!(
            timeline.session.as_ref().unwrap().speech_start,
            1_600 + 1_600
        );
    }

    #[test]
    fn stop_during_a_delivery_pause_waits_for_its_full_endpoint() {
        let mut timeline = Timeline::new(RATE, 32_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 16_000);
        timeline.begin_session(epoch + ms(500), 0, usize::MAX);
        // Delivery paused after 1.0 s of audio; stop arrived at 1.4 s.
        let end = timeline.end_session(epoch + ms(1400), 3_200).unwrap();
        assert_eq!(end, 22_400 + 3_200);
        assert!(!timeline.reached(end));
        feed(&mut timeline, epoch, 9_600);
        assert!(timeline.reached(end));
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.speech_end, 22_400 - 8_000);
        assert_eq!(captured.tail(), 3_200);
    }

    #[test]
    fn cold_session_keeps_every_sample_as_speech() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        // The stream delivered a burst before the recording attached.
        timeline.push(&[0.5, 0.25], epoch);
        timeline.begin_session_at_stream_start(usize::MAX);
        timeline.push(&[0.125], epoch + ms(1));
        timeline.end_session(epoch + ms(1), 0);
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.samples, [0.5, 0.25, 0.125]);
        assert_eq!(captured.preroll, 0);
        assert_eq!(captured.speech_end, 3);
    }

    #[test]
    fn command_older_than_the_ring_keeps_the_ring_without_preroll() {
        let mut timeline = Timeline::new(RATE, 1_600);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 32_000);
        // The handler ran long after the command; its index left the ring.
        let preroll = timeline.begin_session(epoch + ms(100), 7_200, usize::MAX);
        assert_eq!(preroll, 0);
        timeline.end_session(epoch + Duration::from_secs(2), 0);
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.samples.first(), Some(&30_400.0));
        assert_eq!(captured.samples.len(), 1_600);
        assert_eq!(captured.preroll, 0);
        assert_eq!(captured.speech_end, 1_600);
    }

    #[test]
    fn tail_wait_ends_when_the_stream_ends_or_the_buffer_is_full() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 1_600);
        timeline.begin_session(epoch + ms(100), 0, 1_000);
        let end = timeline.end_session(epoch + ms(100), 3_200).unwrap();
        assert!(!timeline.reached(end));
        feed(&mut timeline, epoch, 1_120);
        assert_eq!(timeline.session.as_ref().unwrap().buffer.len(), 1_000);
        assert!(timeline.reached(end));

        let mut ended = Timeline::new(RATE, 16_000);
        ended.begin_session(epoch, 0, usize::MAX);
        let end = ended.end_session(epoch, 3_200).unwrap();
        ended.mark_ended(Some("parec exited".into()));
        assert!(ended.reached(end));
        assert_eq!(ended.error(), Some("parec exited"));
    }

    #[test]
    fn late_stop_handling_truncates_audio_past_the_tail() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 1_600);
        timeline.begin_session(epoch + ms(100), 0, usize::MAX);
        feed(&mut timeline, epoch, 16_000);
        // The stop command arrived at 0.5 s but was handled at 1.1 s.
        timeline.end_session(epoch + ms(500), 3_200);
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.samples.len(), 6_400 + 3_200);
        assert_eq!(captured.speech_end, 6_400);
    }

    fn square(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|index| {
                if index % 2 == 0 {
                    amplitude
                } else {
                    -amplitude
                }
            })
            .collect()
    }

    #[test]
    fn gate_level_removes_dc_offset() {
        assert_eq!(loudest_window_dbfs(&[0.5; 4_800], 480), f64::NEG_INFINITY);
        let offset: Vec<f32> = square(0.1, 4_800).iter().map(|x| x + 0.4).collect();
        assert!((loudest_window_dbfs(&offset, 480) - -20.0).abs() < 1e-3);
        assert_eq!(loudest_window_dbfs(&[], 480), f64::NEG_INFINITY);
    }

    #[test]
    fn gate_level_finds_the_loudest_window_anywhere() {
        let mut samples = vec![0.0; 48_000];
        samples[30_001..30_481].copy_from_slice(&square(0.01, 480));
        assert!((loudest_window_dbfs(&samples, 480) - -40.0).abs() < 1e-3);
        // Running sums stay exact across resynchronization boundaries.
        let long = square(0.001, 1_000_000);
        assert!((loudest_window_dbfs(&long, 480) - -60.0).abs() < 1e-6);
    }

    #[test]
    fn gate_threshold_is_minus_fifty_dbfs() {
        let threshold = 10f32.powf(-50.0 / 20.0);
        let above = loudest_window_dbfs(&square(threshold * 1.01, 9_600), 480);
        let below = loudest_window_dbfs(&square(threshold * 0.99, 9_600), 480);
        assert_eq!(gate(ms(600), above), Verdict::Keep);
        assert_eq!(gate(ms(600), below), Verdict::Silent);
    }

    #[test]
    fn gate_rejects_short_spans_regardless_of_level() {
        assert_eq!(gate(ms(299), 0.0), Verdict::TooShort);
        assert_eq!(gate(MIN_SPEECH, 0.0), Verdict::Keep);
    }

    #[test]
    fn gate_measurement_excludes_the_tail() {
        let mut timeline = Timeline::new(RATE, 16_000);
        let epoch = Instant::now();
        feed(&mut timeline, epoch, 1_600);
        timeline.begin_session(epoch + ms(100), 0, usize::MAX);
        timeline.push(&vec![0.0; 8_000], epoch + ms(600));
        timeline.end_session(epoch + ms(600), 3_200);
        timeline.push(&square(0.5, 3_200), epoch + ms(800));
        let captured = timeline.take_session().unwrap();
        assert_eq!(captured.tail(), 3_200);
        assert!(loudest_window_dbfs(&captured.samples, 480) > -10.0);
        let speech = &captured.samples[..captured.speech_end];
        assert_eq!(loudest_window_dbfs(speech, 480), f64::NEG_INFINITY);
    }

    #[test]
    fn short_input_is_measured_as_one_window() {
        assert!((loudest_window_dbfs(&square(0.1, 100), 480) - -20.0).abs() < 1e-3);
    }
}
