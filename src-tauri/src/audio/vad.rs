use std::path::Path;

/// Simple energy-based Voice Activity Detection processor
///
/// This is a lightweight VAD that detects speech based on audio energy levels.
/// It's not as accurate as neural network-based VAD (like Silero) but has no
/// external dependencies and works well for most speech-to-text use cases.
pub struct VadProcessor {
    /// Energy threshold for speech detection (0.0 to 1.0)
    threshold: f32,
    /// Number of frames to keep before speech starts (pre-speech buffer)
    pre_speech_frames: usize,
    /// Number of frames to keep after speech ends (post-speech buffer)
    post_speech_frames: usize,
    /// Ring buffer for pre-speech audio
    pre_buffer: Vec<Vec<f32>>,
    /// Counter for post-speech frames
    post_speech_counter: usize,
    /// Whether we're currently in speech
    in_speech: bool,
    /// Accumulated speech audio
    speech_audio: Vec<f32>,
    /// Frame size (512 samples = 32ms at 16kHz)
    frame_size: usize,
    /// Running average of background noise energy
    noise_floor: f32,
    /// Adaptation rate for noise floor
    noise_adaptation_rate: f32,
}

impl VadProcessor {
    /// Create a new VAD processor
    ///
    /// # Arguments
    /// * `_model_path` - Unused, kept for API compatibility
    /// * `_sample_rate` - Sample rate of the audio (expected to be 16000)
    pub fn new(_model_path: &Path, _sample_rate: u32) -> Result<Self, String> {
        let frame_size = 512; // 32ms at 16kHz
        let frame_duration_ms = 32;
        let pre_speech_frames = 300 / frame_duration_ms; // ~300ms
        let post_speech_frames = 500 / frame_duration_ms; // ~500ms

        Ok(Self {
            threshold: 0.0003, // Very low threshold - detect speech when energy is 0.0003 above noise floor
            pre_speech_frames,
            post_speech_frames,
            pre_buffer: Vec::with_capacity(pre_speech_frames),
            post_speech_counter: 0,
            in_speech: false,
            speech_audio: Vec::new(),
            frame_size,
            noise_floor: 0.0001, // Very low initial noise floor
            noise_adaptation_rate: 0.005, // Slower adaptation to avoid tracking speech as noise
        })
    }

    /// Calculate RMS energy of audio frame
    fn calculate_energy(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
        (sum_squares / samples.len() as f32).sqrt()
    }

    /// Check if frame contains speech based on energy
    fn is_speech(&mut self, frame: &[f32]) -> bool {
        let energy = self.calculate_energy(frame);
        let threshold_value = self.noise_floor + self.threshold;

        // Adaptive threshold: speech is detected when energy is significantly above noise floor
        let is_speech = energy > threshold_value;

        // Update noise floor estimate during silence
        if !is_speech && !self.in_speech {
            self.noise_floor = self.noise_floor * (1.0 - self.noise_adaptation_rate)
                             + energy * self.noise_adaptation_rate;
            // Keep noise floor in reasonable bounds - very low for quiet mics
            self.noise_floor = self.noise_floor.clamp(0.00005, 0.0005);
        }

        is_speech
    }

    /// Process audio samples and return speech-only audio
    ///
    /// The VAD filters out silence and non-speech segments.
    /// Returns accumulated speech audio when utterance is complete.
    ///
    /// # Arguments
    /// * `samples` - Audio samples at 16kHz mono
    ///
    /// # Returns
    /// * Speech audio if an utterance is complete, empty otherwise
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        let mut output = Vec::new();

        for chunk in samples.chunks(self.frame_size) {
            if chunk.len() < self.frame_size {
                // Pad short chunks with zeros
                let mut padded = chunk.to_vec();
                padded.resize(self.frame_size, 0.0);
                self.process_frame(&padded, &mut output);
            } else {
                self.process_frame(chunk, &mut output);
            }
        }

        output
    }

    /// Process a single frame of audio
    fn process_frame(&mut self, frame: &[f32], output: &mut Vec<f32>) {
        let is_speech = self.is_speech(frame);

        if is_speech {
            if !self.in_speech {
                // Speech started - add pre-buffer
                self.in_speech = true;
                for buffered_frame in &self.pre_buffer {
                    self.speech_audio.extend_from_slice(buffered_frame);
                }
                self.pre_buffer.clear();
            }

            // Add current frame to speech
            self.speech_audio.extend_from_slice(frame);
            self.post_speech_counter = 0;
        } else {
            if self.in_speech {
                // In post-speech buffer period
                self.speech_audio.extend_from_slice(frame);
                self.post_speech_counter += 1;

                if self.post_speech_counter >= self.post_speech_frames {
                    // Speech ended - output accumulated audio
                    output.append(&mut self.speech_audio);
                    self.in_speech = false;
                    self.post_speech_counter = 0;
                }
            } else {
                // Not in speech - maintain pre-buffer
                if self.pre_buffer.len() >= self.pre_speech_frames {
                    self.pre_buffer.remove(0);
                }
                self.pre_buffer.push(frame.to_vec());
            }
        }
    }

    /// Flush any remaining speech audio
    pub fn flush(&mut self) -> Vec<f32> {
        let result = std::mem::take(&mut self.speech_audio);
        self.reset();
        result
    }

    /// Reset the VAD state
    pub fn reset(&mut self) {
        self.pre_buffer.clear();
        self.post_speech_counter = 0;
        self.in_speech = false;
        self.speech_audio.clear();
        self.noise_floor = 0.001;
    }

}
