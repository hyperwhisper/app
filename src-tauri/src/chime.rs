// The two sounds the app makes: one when a recording starts, one when the
// text is ready. Worked out sample by sample rather than played from a file,
// so there is nothing extra to ship or sign - cpal is already here for the
// microphone.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat};
use std::thread;
use std::time::Duration;

// One note of a chime.
pub(crate) struct Note {
    hz: f32,
    /// Seconds after the chime begins before this note starts.
    delay: f32,
    seconds: f32,
}

// Recording started: G4 down to C4, a calm descending fifth in a lower
// register. Kept short so it does not sit on top of your first words.
pub(crate) static START_CHIME: [Note; 2] = [
    Note {
        hz: 392.0,
        delay: 0.0,
        seconds: 0.30,
    },
    Note {
        hz: 261.63,
        delay: 0.08,
        seconds: 0.42,
    },
];

// Text is ready: C5 up to E5, a calming ascending major third.
pub(crate) static DONE_CHIME: [Note; 2] = [
    Note {
        hz: 523.25,
        delay: 0.0,
        seconds: 0.40,
    },
    Note {
        hz: 659.25,
        delay: 0.08,
        seconds: 0.52,
    },
];

// How loud one note is this far into itself: quick fade in, long fade out. A
// note that starts or stops instantly clicks instead of sounding soft.
pub(crate) fn note_loudness(age: f32, seconds: f32) -> f32 {
    const PEAK: f32 = 0.12;
    const FADE_IN: f32 = 0.03;
    if age < 0.0 || age > seconds {
        return 0.0;
    }
    if age < FADE_IN {
        return PEAK * age / FADE_IN;
    }
    let fade_out = (seconds - FADE_IN).max(0.001);
    PEAK * (1.0 - (age - FADE_IN) / fade_out).max(0.0)
}

// The whole chime as one number, this many seconds in.
pub(crate) fn chime_at(notes: &[Note], seconds: f32) -> f32 {
    notes
        .iter()
        .map(|note| {
            let age = seconds - note.delay;
            (2.0 * std::f32::consts::PI * note.hz * age).sin() * note_loudness(age, note.seconds)
        })
        .sum()
}

pub(crate) fn chime_seconds(notes: &[Note]) -> f32 {
    notes
        .iter()
        .fold(0.0f32, |longest, n| longest.max(n.delay + n.seconds))
}

pub(crate) fn build_chime_stream<T>(
    device: &Device,
    config: &cpal::StreamConfig,
    notes: &'static [Note],
    sample_rate: f32,
    channels: usize,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut frame = 0usize;
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for slot in data.chunks_mut(channels.max(1)) {
                let value = T::from_sample(chime_at(notes, frame as f32 / sample_rate));
                for sample in slot.iter_mut() {
                    *sample = value;
                }
                frame += 1;
            }
        },
        |e| eprintln!("chime: {}", e),
        None,
    )
}

// The two cues the app makes: one when recording starts, one when the text is
// ready. Worked out sample by sample rather than played from a sound file, so
// there is nothing extra to ship or sign - cpal is already here for the
// microphone. Runs on its own thread: nothing should wait for a sound.
pub(crate) fn play_chime(notes: &'static [Note]) {
    thread::spawn(move || {
        let Some(device) = cpal::default_host().default_output_device() else {
            eprintln!("chime: no speakers to play it through");
            return;
        };
        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("chime: the speakers would not say what they accept ({})", e);
                return;
            }
        };
        let format = config.sample_format();
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        let stream_config: cpal::StreamConfig = config.into();

        let stream = match format {
            SampleFormat::F32 => {
                build_chime_stream::<f32>(&device, &stream_config, notes, sample_rate, channels)
            }
            SampleFormat::I16 => {
                build_chime_stream::<i16>(&device, &stream_config, notes, sample_rate, channels)
            }
            SampleFormat::U16 => {
                build_chime_stream::<u16>(&device, &stream_config, notes, sample_rate, channels)
            }
            other => {
                eprintln!(
                    "chime: the speakers use a format this cannot make ({:?})",
                    other
                );
                return;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("chime: the speakers would not open ({})", e);
                return;
            }
        };
        if let Err(e) = stream.play() {
            eprintln!("chime: the speakers would not start ({})", e);
            return;
        }
        // The stream is dropped, and so silenced, when this thread ends.
        thread::sleep(Duration::from_secs_f32(chime_seconds(notes) + 0.2));
    });
}
