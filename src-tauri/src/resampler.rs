use rubato::{FftFixedIn, Resampler};

/// Audio resampler for converting device sample rate to 16kHz for parakeet-rs
pub struct AudioResampler {
    resampler: FftFixedIn<f32>,
    input_buffer: Vec<f32>,
    chunk_size: usize,
}

impl AudioResampler {
    /// Create a new resampler from source sample rate to 16kHz
    pub fn new(source_rate: u32) -> Result<Self, String> {
        const TARGET_RATE: u32 = 16000;

        // Use a chunk size that gives good latency (~10ms at source rate)
        let chunk_size = (source_rate as usize) / 100;

        let resampler = FftFixedIn::<f32>::new(
            source_rate as usize,
            TARGET_RATE as usize,
            chunk_size,
            2, // sub_chunks for better quality
            1, // mono
        )
        .map_err(|e| format!("Failed to create resampler: {}", e))?;

        Ok(Self {
            resampler,
            input_buffer: Vec::new(),
            chunk_size,
        })
    }

    /// Process audio samples, returning resampled output when enough data is available
    pub fn process(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        self.input_buffer.extend_from_slice(samples);

        let mut output = Vec::new();

        // Process complete chunks
        while self.input_buffer.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.input_buffer.drain(..self.chunk_size).collect();
            let input_frames = vec![chunk];

            let resampled = self.resampler
                .process(&input_frames, None)
                .map_err(|e| format!("Resampling failed: {}", e))?;

            if !resampled.is_empty() && !resampled[0].is_empty() {
                output.extend_from_slice(&resampled[0]);
            }
        }

        Ok(output)
    }

    /// Flush any remaining samples
    pub fn flush(&mut self) -> Result<Vec<f32>, String> {
        if self.input_buffer.is_empty() {
            return Ok(Vec::new());
        }

        // Pad with zeros to complete the chunk
        let remaining = self.input_buffer.len();
        let padding_needed = self.chunk_size - remaining;
        self.input_buffer.extend(vec![0.0f32; padding_needed]);

        let chunk: Vec<f32> = self.input_buffer.drain(..).collect();
        let input_frames = vec![chunk];

        let resampled = self.resampler
            .process(&input_frames, None)
            .map_err(|e| format!("Resampling failed: {}", e))?;

        if !resampled.is_empty() && !resampled[0].is_empty() {
            // Only return the non-padded portion (approximately)
            let expected_samples = (remaining as f32 * 16000.0 / self.chunk_size as f32) as usize;
            let actual = resampled[0].len().min(expected_samples.max(1));
            Ok(resampled[0][..actual].to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}
