//! Splitting a continuous stream into complete utterances.
//!
//! This module owns the decision that makes the whole count honest. A word is
//! counted only once, from an utterance that can no longer change, so no
//! recogniser ever re-reads the same audio.
//!
//! The alternative — feed a rolling window to whisper and count whatever comes
//! back — overcounts, because consecutive windows overlap and report the same
//! sentence twice. That inflates the total quietly, which for a yapathon is the
//! one failure that cannot be noticed or forgiven.
//!
//! So the rule is: **buffer until silence, then transcribe, then discard.**

use yap_core::engine::{SAMPLE_RATE, VadConfig, is_loud_enough, samples_for_ms};

/// What the segmenter has decided about the audio it is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not enough energy to be speech.
    Silence,
    /// Speech is running; more may be appended.
    Speech,
}

/// Splits 16 kHz mono audio into utterances.
///
/// A pure state machine over sample buffers: no audio library, no threads, no
/// clock. Feed it samples and it tells you when an utterance is complete. That
/// makes the counting rule testable without a microphone, which matters because
/// this is the part that has to be right.
#[derive(Debug)]
pub struct Segmenter {
    config: VadConfig,
    phase: Phase,
    /// Audio since the last emitted utterance, speech and its padding included.
    buffer: Vec<f32>,
    /// Position of the first sample of the current utterance in the stream.
    cursor_ms: u64,
    /// Samples accepted so far, for timestamp arithmetic.
    total_samples: u64,
    /// Trailing silence retained in case speech resumes. Held separately so a
    /// long pause does not grow `buffer` without bound.
    trailing_silence: Vec<f32>,
}

impl Segmenter {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            phase: Phase::Silence,
            buffer: Vec::new(),
            cursor_ms: 0,
            total_samples: 0,
            trailing_silence: Vec::new(),
        }
    }

    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// How much audio is currently held, in milliseconds.
    pub fn buffered_ms(&self) -> u64 {
        self.buffer.len() as u64 * 1000 / u64::from(SAMPLE_RATE)
    }

    /// Feed samples, returning any utterances they completed.
    ///
    /// Returns more than one when a long silence separates two bursts. Order is
    /// always chronological, because the goal depends on a total that matches
    /// the order words were spoken.
    pub fn push(&mut self, samples: &[f32]) -> Vec<SegmentedUtterance> {
        let mut out = Vec::new();

        // Process in fixed frames so behaviour does not depend on the capture
        // callback's buffer size, which varies by device.
        for frame in samples.chunks(FRAME_SAMPLES).filter(|f| !f.is_empty()) {
            self.push_frame(frame, &mut out);
        }

        out
    }

    fn push_frame(&mut self, frame: &[f32], out: &mut Vec<SegmentedUtterance>) {
        // A loud frame while buffering speech extends it. Checked first so a
        // speech frame that also follows the max length still ends the
        // utterance at the cap.
        let loud = is_loud_enough(frame, self.config.energy_floor);

        match self.phase {
            Phase::Silence => {
                if loud {
                    self.begin_speech();
                    self.append(frame);
                }
                // Silence outside an utterance is dropped. Retaining it would
                // grow memory for the length of the stream and could let a
                // later utterance start with stale audio.
                self.advance(frame.len());
            }
            Phase::Speech => {
                let already_full = self.buffered_ms() >= u64::from(self.config.max_speech_ms);

                if loud {
                    // Speech resumed inside the padding window: this was a
                    // breath, not an end of utterance.
                    self.extend_speech(frame);
                } else {
                    self.trailing_silence.extend_from_slice(frame);

                    // Enough silence to call it: emit, then start fresh.
                    if self.silence_ms() >= u64::from(self.config.min_silence_ms) {
                        self.emit(out);
                        self.advance(frame.len());
                    }
                    return;
                }

                if already_full {
                    // Hit the cap mid-sentence. Splitting is better than one
                    // window whisper would truncate, and the count stays right
                    // because the halves do not overlap.
                    self.emit(out);
                }

                self.advance(frame.len());
            }
        }
    }

    /// Enter the speech phase, seeding the buffer with any retained padding so
    /// a consonant at the very start of the utterance is not clipped.
    fn begin_speech(&mut self) {
        self.phase = Phase::Speech;

        let padding = samples_for_ms(self.config.speech_pad_ms);
        if !self.trailing_silence.is_empty() {
            let take = padding.min(self.trailing_silence.len());
            let start = self.trailing_silence.len() - take;
            let seeded: Vec<f32> = self.trailing_silence[start..].to_vec();
            self.buffer.extend_from_slice(&seeded);
            self.trailing_silence.clear();
        }

        // The cursor moves back to where the utterance really began, so the
        // recorded timestamp matches the audio rather than the decision.
        let seeded_ms = self.buffer.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        self.cursor_ms = self.cursor_ms.saturating_sub(seeded_ms);
    }

    /// Continue an utterance through a pause shorter than `min_silence_ms`.
    ///
    /// Only the last `speech_pad_ms` of the pause is kept. Holding all of it
    /// would pad the transcript with a breath the streamer did not intend as a
    /// break, and would stretch the recorded duration well past the speech.
    fn extend_speech(&mut self, frame: &[f32]) {
        let padding = samples_for_ms(self.config.speech_pad_ms);
        if self.trailing_silence.len() > padding {
            let split = self.trailing_silence.len() - padding;
            self.buffer
                .extend_from_slice(&self.trailing_silence[split..]);
        } else {
            self.buffer.extend_from_slice(&self.trailing_silence);
        }
        self.trailing_silence.clear();
        self.append(frame);
    }

    fn append(&mut self, frame: &[f32]) {
        self.buffer.extend_from_slice(frame);
    }

    /// Trailing silence held so far.
    fn silence_ms(&self) -> u64 {
        self.trailing_silence.len() as u64 * 1000 / u64::from(SAMPLE_RATE)
    }

    /// Close the current utterance if it is worth transcribing.
    ///
    /// Short bursts are dropped rather than sent to an engine: whisper will
    /// transcribe a cough as a short sentence, and those words would count.
    fn emit(&mut self, out: &mut Vec<SegmentedUtterance>) {
        self.phase = Phase::Silence;

        // Trailing silence is not speech; the pad was already added at the start.
        self.trailing_silence.clear();

        let speech_ms = self.buffered_ms();
        if speech_ms < u64::from(self.config.min_speech_ms) {
            self.buffer.clear();
            return;
        }

        let samples = std::mem::take(&mut self.buffer);
        let started_ms = self.cursor_ms;
        let ended_ms = self.cursor_ms + speech_ms;

        out.push(SegmentedUtterance {
            samples,
            started_ms,
            ended_ms,
        });

        self.cursor_ms = ended_ms;
    }

    fn advance(&mut self, samples: usize) {
        self.total_samples += samples as u64;
        // Only move the cursor when no utterance is open, so timestamps stay
        // anchored to the speech rather than to when the frame arrived.
        if self.phase == Phase::Silence && self.buffer.is_empty() {
            self.cursor_ms = self.total_samples * 1000 / u64::from(SAMPLE_RATE);
        }
    }

    /// Flush the held utterance, if any.
    ///
    /// Called when the session pauses or ends, so words spoken just before the
    /// streamer stopped are not lost to a silence that had not arrived yet.
    pub fn flush(&mut self) -> Option<SegmentedUtterance> {
        if self.buffer.is_empty() {
            return None;
        }

        let mut out = Vec::new();
        self.emit(&mut out);
        out.pop()
    }

    /// Drop everything held, without emitting. Used on pause.
    pub fn reset(&mut self) {
        self.phase = Phase::Silence;
        self.buffer.clear();
        self.trailing_silence.clear();
        self.cursor_ms = self.total_samples * 1000 / u64::from(SAMPLE_RATE);
    }
}

/// One complete utterance, ready for an engine.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentedUtterance {
    /// 16 kHz mono samples.
    pub samples: Vec<f32>,
    pub started_ms: u64,
    pub ended_ms: u64,
}

impl SegmentedUtterance {
    pub fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }

    /// Convert into the core's engine-facing type.
    pub fn into_engine(self) -> yap_core::engine::Utterance {
        yap_core::engine::Utterance {
            samples: self.samples,
            started_ms: self.started_ms,
            ended_ms: self.ended_ms,
        }
    }
}

/// Frame size in samples: 32 ms at 16 kHz.
///
/// Short enough to place an utterance boundary within a reasonable distance of
/// the actual pause, long enough not to re-evaluate energy thousands of times
/// a second.
const FRAME_SAMPLES: usize = 512;

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of tone at `amplitude`, which is speech by the energy floor.
    fn tone(samples: usize, amplitude: f32) -> Vec<f32> {
        vec![amplitude; samples]
    }

    /// A frame of near-silence, below the energy floor.
    fn quiet(samples: usize) -> Vec<f32> {
        vec![0.0005f32; samples]
    }

    /// 32 ms of samples.
    fn frame() -> usize {
        FRAME_SAMPLES
    }

    fn config() -> VadConfig {
        VadConfig::default()
    }

    #[test]
    fn silence_never_emits() {
        let mut s = Segmenter::new(config());
        let out = s.push(&quiet(frame() * 100));
        assert!(out.is_empty());
        assert_eq!(s.buffered_ms(), 0);
    }

    #[test]
    fn speech_needs_a_following_silence_to_be_an_utterance() {
        let mut s = Segmenter::new(config());

        // Two seconds of speech and no silence: still buffered, nothing counted.
        let out = s.push(&tone(frame() * 62, 0.3));
        assert!(
            out.is_empty(),
            "an unterminated utterance must not be emitted"
        );
        assert!(s.buffered_ms() > 1_500);

        // Half a second of silence closes it.
        let out = s.push(&quiet(frame() * 16));
        assert_eq!(out.len(), 1);

        let u = &out[0];
        assert_eq!(
            u.samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE),
            u.duration_ms()
        );
        assert!(u.duration_ms() >= 1_500, "got {}ms", u.duration_ms());
    }

    #[test]
    fn the_same_audio_is_counted_once() {
        let mut s = Segmenter::new(config());
        let mut emitted = 0;

        // Push the same two-second burst ten times, each followed by a pause.
        for _ in 0..10 {
            emitted += s.push(&tone(frame() * 62, 0.3)).len();
            emitted += s.push(&quiet(frame() * 20)).len();
        }

        // Ten distinct utterances, not one counted repeatedly and not the same
        // audio emitted twice by an overlapping window.
        assert_eq!(emitted, 10);
    }

    #[test]
    fn a_short_burst_is_discarded_rather_than_transcribed() {
        let mut s = Segmenter::new(config());

        // ~64 ms of speech, under the 200 ms minimum: a cough or a door.
        let out = s.push(&tone(1000, 0.4));
        assert!(out.is_empty());

        let out = s.push(&quiet(frame() * 20));
        assert!(
            out.is_empty(),
            "a cough must not reach the engine and become words"
        );
    }

    #[test]
    fn a_breath_does_not_split_an_utterance() {
        let mut s = Segmenter::new(config());

        // One second of speech, a short pause under the silence threshold,
        // then more speech.
        let out = s.push(&tone(frame() * 31, 0.3));
        assert!(out.is_empty());
        let out = s.push(&quiet(frame() * 3)); // ~96 ms pause
        assert!(out.is_empty(), "a breath is not an end of sentence");

        let out = s.push(&tone(frame() * 31, 0.3));
        assert!(out.is_empty());

        let out = s.push(&quiet(frame() * 20));
        assert_eq!(out.len(), 1, "the two halves are one utterance");

        // Roughly two seconds of speech survived intact.
        assert!(
            out[0].duration_ms() >= 1_800,
            "got {}ms",
            out[0].duration_ms()
        );
    }

    #[test]
    fn timestamps_are_chronological_and_gapless() {
        let mut s = Segmenter::new(config());
        let mut utterances = Vec::new();

        for _ in 0..3 {
            utterances.extend(s.push(&tone(frame() * 62, 0.3)));
            utterances.extend(s.push(&quiet(frame() * 25)));
        }

        assert_eq!(utterances.len(), 3);
        for pair in utterances.windows(2) {
            assert!(
                pair[0].ended_ms <= pair[1].started_ms,
                "utterances must not overlap: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn an_over_long_utterance_is_split_at_the_cap() {
        let mut c = config();
        c.max_speech_ms = 500;
        let mut s = Segmenter::new(c);

        // Three seconds of unbroken speech.
        let out = s.push(&tone(frame() * 94, 0.3));
        assert!(out.len() >= 2, "expected a split, got {}", out.len());

        // Each piece respects the cap.
        for u in &out {
            assert!(u.duration_ms() <= 700, "piece was {}ms", u.duration_ms());
        }
    }

    #[test]
    fn flush_emits_audio_still_being_held() {
        let mut s = Segmenter::new(config());
        s.push(&tone(frame() * 62, 0.3));

        // The streamer stops mid-sentence: no silence has arrived yet.
        let flushed = s.flush().expect("held audio is flushed");
        assert!(flushed.duration_ms() >= 1_500);

        // Flushing again has nothing left to give.
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn flush_ignores_held_silence() {
        let mut s = Segmenter::new(config());
        s.push(&tone(frame() * 62, 0.3));
        s.push(&quiet(frame() * 20));
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn reset_drops_held_audio() {
        let mut s = Segmenter::new(config());
        s.push(&tone(frame() * 62, 0.3));
        s.reset();

        assert_eq!(s.buffered_ms(), 0);
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn frame_boundaries_do_not_change_the_result() {
        // The same audio, delivered in different chunk sizes, must segment the
        // same way. Capture callback sizes vary by device and OS.
        let speech = tone(frame() * 62, 0.3);
        let silence = quiet(frame() * 25);

        let mut whole = Segmenter::new(config());
        let a = whole.push(&[&speech[..], &silence[..]].concat());

        let mut chunked = Segmenter::new(config());
        let mut b = Vec::new();
        for chunk in speech.chunks(777).chain(silence.chunks(333)) {
            b.extend(chunked.push(chunk));
        }

        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        // Timestamps agree to within a frame.
        assert!((a[0].started_ms as i64 - b[0].started_ms as i64).abs() < 64);
    }

    #[test]
    fn a_long_pause_does_not_end_up_in_the_utterance() {
        let mut s = Segmenter::new(config());
        let mut emitted = s.push(&tone(frame() * 62, 0.3));

        // A minute of silence after the speech. The utterance closes early, at
        // the first pause long enough to be a boundary — the rest of the
        // silence must not be retained and prepended to a later utterance.
        for _ in 0..600 {
            emitted.extend(s.push(&quiet(frame())));
        }
        emitted.extend(s.push(&quiet(frame())));

        assert_eq!(emitted.len(), 1, "the pause should close it exactly once");

        // What was retained is the speech and its pad, not the minute of quiet.
        assert!(
            emitted[0].duration_ms() < 3_000,
            "retained {}ms",
            emitted[0].duration_ms()
        );

        // And the minute of silence did not become the next utterance either.
        assert_eq!(s.flush(), None);
    }
}
