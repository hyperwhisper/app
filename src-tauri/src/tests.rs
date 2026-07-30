// Tests for the maths the app depends on: turning what the microphone sends
// into numbers, finding the speech in a recording, and the settings file.
// No microphone, no model, no windows.
use crate::analysis::*;
use crate::chime::*;
use crate::settings::*;
use crate::storage::delete_recordings_in;
use crate::*;
use std::fs;

fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: got {got}, wanted {want} (allowed {tolerance})"
    );
}

// Blocks of 480 samples (30 ms at 16 kHz), alternating +/- amplitude so the
// measured loudness is exactly that amplitude.
fn blocks(spec: &[(usize, f32)]) -> Vec<f32> {
    let mut out = Vec::new();
    for &(count, amplitude) in spec {
        for _ in 0..count {
            for i in 0..480 {
                out.push(if i % 2 == 0 { amplitude } else { -amplitude });
            }
        }
    }
    out
}

fn sine(count: usize, hz: f32, sample_rate: f32, amplitude: f32) -> Vec<f32> {
    (0..count)
        .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sample_rate).sin() * amplitude)
        .collect()
}

// Repeatable stand-in for noise, so a failure can be reproduced.
fn noise(count: usize, amplitude: f32) -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let unit = (state >> 32) as f32 / u32::MAX as f32;
            (unit * 2.0 - 1.0) * amplitude
        })
        .collect()
}

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |max, s| max.max(s.abs()))
}

// ---- turning what the device sends into numbers between -1 and 1 -------

#[test]
fn sixteen_bit_signed_samples_convert() {
    // (raw value from the device, expected, what this case is for)
    let cases: &[(i16, f32, &str)] = &[
        (0, 0.0, "silence sits at zero"),
        (i16::MAX, 1.0, "the loudest positive value reaches 1.0"),
        (-i16::MAX, -1.0, "the loudest negative value reaches -1.0"),
        (16383, 0.5, "half loudness"),
        (-16383, -0.5, "half loudness, negative"),
    ];
    for &(raw, want, what) in cases {
        near(i16_to_f32(raw), want, 0.0001, what);
    }
    // One value below the loudest negative. It is allowed to land slightly
    // past -1.0; the check that follows every boost keeps it in range.
    assert!(i16_to_f32(i16::MIN) >= -1.001);
}

#[test]
fn sixteen_bit_unsigned_samples_convert() {
    let cases: &[(u16, f32, &str)] = &[
        (32768, 0.0, "silence sits in the middle, not at zero"),
        (0, -1.0, "the bottom of the range is the loudest negative"),
        (
            u16::MAX,
            1.0,
            "the top of the range is the loudest positive",
        ),
        (49151, 0.5, "half loudness"),
        (16384, -0.5, "half loudness, negative"),
    ];
    for &(raw, want, what) in cases {
        near(u16_to_f32(raw), want, 0.0001, what);
    }
}

#[test]
fn silence_from_an_unsigned_device_stays_silent() {
    // Scaling without shifting would turn a silent room into a steady tone.
    let quiet: Vec<f32> = vec![32768u16; 4800].into_iter().map(u16_to_f32).collect();
    near(peak_of(&quiet), 0.0, 0.0001, "silence must stay silent");
    assert!(!holds_speech(speech_level(&quiet), peak_of(&quiet)));
}

// ---- mixing several channels down to one ------------------------------

#[test]
fn channels_are_averaged_into_one() {
    // (channels, what the device sent, what should come out, why)
    let cases: &[(u16, &[f32], &[f32], &str)] = &[
        (
            1,
            &[0.1, 0.2, 0.3],
            &[0.1, 0.2, 0.3],
            "one channel passes through",
        ),
        (
            0,
            &[0.1, 0.2],
            &[0.1, 0.2],
            "a nonsense channel count changes nothing",
        ),
        (
            2,
            &[1.0, 0.0, 0.5, 0.5],
            &[0.5, 0.5],
            "two channels are averaged",
        ),
        (
            2,
            &[1.0, -1.0],
            &[0.0],
            "opposite channels cancel, they are not just dropped",
        ),
        (3, &[0.3, 0.6, 0.9], &[0.6], "three channels are averaged"),
        (
            2,
            &[1.0, 0.0, 1.0],
            &[0.5, 1.0],
            "a half-finished frame at the end is kept",
        ),
        (2, &[], &[], "nothing in, nothing out"),
    ];
    for &(channels, input, want, what) in cases {
        let got = mix_to_mono(input.to_vec(), channels);
        assert_eq!(got.len(), want.len(), "{what}: wrong length");
        for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
            near(g, w, 0.0001, &format!("{what}, sample {i}"));
        }
    }
}

// ---- how loud is the speech -------------------------------------------

#[test]
fn speech_loudness_ignores_pauses_and_single_bangs() {
    // (recording, expected loudness, why this case exists)
    let cases: &[(Vec<f32>, f32, &str)] = &[
        (Vec::new(), 0.0, "nothing recorded"),
        (vec![0.5; 100], 0.0, "shorter than one 30 ms block"),
        (
            blocks(&[(1, 0.3)]),
            0.3,
            "exactly one block reports its own loudness",
        ),
        (
            blocks(&[(50, 0.08)]),
            0.08,
            "steady speech reports its loudness",
        ),
        (
            blocks(&[(99, 0.002), (1, 0.9)]),
            0.002,
            "one door slam in a quiet minute does not count as speech",
        ),
        (
            blocks(&[(80, 0.002), (20, 0.08)]),
            0.08,
            "real speech among pauses does count",
        ),
    ];
    for (recording, want, what) in cases {
        near(speech_level(recording), *want, 0.0001, what);
    }
}

#[test]
fn a_single_block_does_not_wrap_to_the_quietest_one() {
    near(
        speech_level(&blocks(&[(1, 0.42)])),
        0.42,
        0.0001,
        "one block",
    );
}

// ---- the rule that silence must never reach the model -----------------

#[test]
fn only_real_speech_is_sent_to_the_model() {
    // (speech loudness, loudest sample, may it be sent, why)
    let cases: &[(f32, f32, bool, &str)] = &[
        (0.0, 0.0, false, "digital silence"),
        (
            0.0014,
            0.02,
            false,
            "an empty room, as measured on this Mac",
        ),
        (
            0.004,
            0.60,
            false,
            "one click: loud peak, but nothing is being said",
        ),
        (0.08, 0.02, false, "a steady hum: high average, no peaks"),
        (0.005, 0.05, true, "exactly on both thresholds"),
        (0.08, 0.50, true, "normal speech close to the microphone"),
        (0.30, 0.99, true, "loud speech"),
    ];
    for &(level, peak, want, what) in cases {
        assert_eq!(holds_speech(level, peak), want, "{what}");
    }
}

// ---- raising the level of a quiet recording ---------------------------

#[test]
fn quiet_recordings_are_raised_but_an_empty_room_is_not() {
    // (recording, expected gain, why)
    // A gain of 1.0 means the recording was left exactly as it was.
    let cases: &[(Vec<f32>, f32, &str)] = &[
        (Vec::new(), 1.0, "nothing recorded"),
        (blocks(&[(10, 0.0)]), 1.0, "digital silence is never raised"),
        (
            blocks(&[(10, 0.002)]),
            1.0,
            "an empty room stays quiet - raising it is how invented sentences got typed",
        ),
        (
            blocks(&[(10, 0.02)]),
            4.0,
            "quiet speech is brought up to normal",
        ),
        (
            blocks(&[(10, 0.006)]),
            10.0,
            "very quiet speech stops at ten times",
        ),
        (
            blocks(&[(10, 0.2)]),
            1.0,
            "speech that is already loud is left alone",
        ),
    ];
    for (recording, want, what) in cases {
        let mut audio = recording.clone();
        near(boost_quiet_audio(&mut audio), *want, 0.01, what);
    }
}

#[test]
fn raising_the_level_never_pushes_a_sample_past_the_limit() {
    let recordings: &[(Vec<f32>, &str)] = &[
        (blocks(&[(10, 0.006)]), "very quiet speech"),
        (blocks(&[(10, 0.02)]), "quiet speech"),
        (
            blocks(&[(99, 0.006), (1, 0.9)]),
            "quiet speech with one loud bang in it",
        ),
        (sine(48_000, 220.0, 16_000.0, 0.01), "a quiet steady tone"),
        (noise(48_000, 0.01), "quiet noise"),
    ];
    for (recording, what) in recordings {
        let mut audio = recording.clone();
        boost_quiet_audio(&mut audio);
        let peak = peak_of(&audio);
        assert!(peak <= 1.0, "{what}: loudest sample reached {peak}");
    }
}

#[test]
fn the_level_after_raising_matches_the_gain_reported() {
    let mut audio = blocks(&[(10, 0.02)]);
    let before = speech_level(&audio);
    let gain = boost_quiet_audio(&mut audio);
    let after = speech_level(&audio);
    near(
        after,
        before * gain,
        0.001,
        "reported gain matches the result",
    );
    near(
        after,
        0.08,
        0.001,
        "quiet speech ends up at normal speech loudness",
    );
}

// ---- cutting the quiet start and end off ------------------------------

#[test]
fn only_the_quiet_start_and_end_are_cut() {
    // 40 blocks of silence, 40 of speech, 40 of silence.
    let mut audio = blocks(&[(40, 0.0), (40, 0.1), (40, 0.0)]);
    let speech_samples_before = audio.iter().filter(|s| s.abs() > 0.05).count();

    let cut = trim_quiet_edges(&mut audio);

    near(cut, 0.72, 0.01, "seconds removed from the front");
    assert_eq!(audio.len(), 34_560, "length after trimming");
    assert_eq!(
        audio.iter().filter(|s| s.abs() > 0.05).count(),
        speech_samples_before,
        "every spoken sample survived the trim"
    );
}

#[test]
fn a_pause_in_the_middle_of_a_sentence_is_never_cut() {
    // Cutting here would join words that were seconds apart.
    let mut audio = blocks(&[(20, 0.1), (40, 0.0), (20, 0.1)]);
    let length_before = audio.len();

    let cut = trim_quiet_edges(&mut audio);

    near(cut, 0.0, 0.0001, "nothing cut from the front");
    assert_eq!(audio.len(), length_before, "nothing cut at all");
    assert_eq!(
        audio.iter().filter(|s| s.abs() < 0.05).count(),
        40 * 480,
        "the pause is still there, at its full length"
    );
}

#[test]
fn trimming_leaves_a_recording_with_no_speech_alone() {
    // (recording, why)
    let cases: &[(Vec<f32>, &str)] = &[
        (blocks(&[(20, 0.0)]), "digital silence"),
        (blocks(&[(20, 0.002)]), "an empty room"),
    ];
    for (recording, what) in cases {
        let mut audio = recording.clone();
        let length_before = audio.len();
        near(trim_quiet_edges(&mut audio), 0.0, 0.0001, what);
        assert_eq!(audio.len(), length_before, "{what}: nothing should be cut");
    }
}

#[test]
fn a_quiet_tail_is_cut_without_touching_the_front() {
    let mut audio = blocks(&[(40, 0.1), (40, 0.0)]);
    let cut = trim_quiet_edges(&mut audio);
    near(cut, 0.0, 0.0001, "nothing cut from the front");
    assert_eq!(audio.len(), 26_880, "the tail was cut");
}

// ---- the numbers shown while recording --------------------------------

#[test]
fn chunk_loudness_reports_peak_and_average() {
    // (chunk, expected loudest, expected average, why)
    let cases: &[(&[f32], f32, f32, &str)] = &[
        (&[], 0.0, 0.0, "nothing recorded"),
        (&[0.0; 8], 0.0, 0.0, "silence"),
        (&[0.5, -0.5, 0.5, -0.5], 0.5, 0.5, "a steady tone"),
        (&[1.0, 0.0, 0.0, 0.0], 1.0, 0.5, "one spike among silence"),
        (
            &[-0.8, 0.1],
            0.8,
            0.5701,
            "the loudest sample can be negative",
        ),
    ];
    for &(chunk, want_peak, want_rms, what) in cases {
        let (peak, rms) = chunk_level(chunk);
        near(peak, want_peak, 0.001, &format!("{what}: loudest"));
        near(rms, want_rms, 0.001, &format!("{what}: average"));
    }
}

#[test]
fn recording_statistics_report_how_much_was_near_silence() {
    // (recording, loudest, average, share near silence, why)
    let cases: &[(Vec<f32>, f32, f32, f32, &str)] = &[
        (
            Vec::new(),
            0.0,
            0.0,
            1.0,
            "nothing recorded counts as all silence",
        ),
        (vec![0.0; 100], 0.0, 0.0, 1.0, "silence"),
        (vec![0.5; 100], 0.5, 0.5, 0.0, "a steady loud signal"),
        (
            [vec![0.5; 50], vec![0.001; 50]].concat(),
            0.5,
            0.3536,
            0.5,
            "half loud, half near silence",
        ),
    ];
    for (recording, want_peak, want_rms, want_quiet, what) in cases {
        let (peak, rms, quiet) = audio_stats(recording);
        near(peak, *want_peak, 0.001, &format!("{what}: loudest"));
        near(rms, *want_rms, 0.001, &format!("{what}: average"));
        near(
            quiet,
            *want_quiet,
            0.001,
            &format!("{what}: share near silence"),
        );
    }
}

#[test]
fn the_recent_sample_store_keeps_the_newest_and_stays_bounded() {
    let store = Arc::new(Mutex::new(Vec::<f32>::new()));

    // Less than the cap: everything is kept.
    keep_recent(&store, &[1.0, 2.0, 3.0]);
    assert_eq!(store.lock().unwrap().len(), 3);

    // Well past the cap, fed in pieces the way the microphone delivers it.
    let total = 6000usize;
    store.lock().unwrap().clear();
    for start in (0..total).step_by(1000) {
        let chunk: Vec<f32> = (start..start + 1000).map(|i| i as f32).collect();
        keep_recent(&store, &chunk);
    }
    let kept = store.lock().unwrap().clone();
    assert_eq!(kept.len(), 4096, "the store must not grow without limit");
    near(kept[4095], 5999.0, 0.5, "the newest sample is kept");
    near(
        kept[0],
        (total - 4096) as f32,
        0.5,
        "the oldest ones are dropped",
    );
}

// ---- the bars and the pitch the windows draw --------------------------

#[test]
fn frequency_bands_stay_in_range_and_follow_the_tone() {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    // Too short to measure: a full set of empty bars, not a crash.
    let short = frequency_bands(&[0.1; 100], &fft);
    assert_eq!(short.len(), BAND_COUNT);
    assert!(
        short.iter().all(|&b| b == 0.0),
        "too-short input gives no bars"
    );

    // Silence: every bar empty.
    let silent = frequency_bands(&vec![0.0; FFT_SIZE], &fft);
    assert!(silent.iter().all(|&b| b == 0.0), "silence gives no bars");

    // A single tone lights up one region, and no bar leaves the 0-to-1 range.
    let tone = sine(FFT_SIZE, 440.0, 16_000.0, 0.5);
    let bands = frequency_bands(&tone, &fft);
    assert_eq!(bands.len(), BAND_COUNT);
    assert!(
        bands.iter().all(|&b| (0.0..=1.0).contains(&b)),
        "a bar outside 0 to 1 would draw off the window"
    );
    let loudest = bands
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    assert!(
        (28..=42).contains(&loudest),
        "a 440 Hz tone should light up the middle of the display, not bar {loudest}"
    );
    assert!(
        bands[0] < bands[loudest] * 0.5,
        "the lowest bar should stay quiet"
    );
}

#[test]
fn pitch_is_refused_when_it_cannot_be_told() {
    // 0.0 means "cannot be told" - better than a wrong number on screen.
    let cases: &[(Vec<f32>, u32, &str)] = &[
        (vec![0.0; 2048], 48_000, "silence"),
        (vec![0.1; 200], 48_000, "too little audio to measure"),
        (
            sine(2048, 200.0, 48_000.0, 0.001),
            48_000,
            "too quiet to measure",
        ),
        (noise(2048, 0.2), 48_000, "noise is not a voice"),
        (noise(2048, 0.5), 16_000, "loud noise is still not a voice"),
    ];
    for (audio, sample_rate, what) in cases {
        near(detect_pitch(audio, *sample_rate), 0.0, 0.0, what);
    }
}

#[test]
fn pitch_is_found_for_a_voice() {
    // (audio, sample rate, expected pitch, allowed error, why)
    // The ignored test below covers the ones this gets wrong.
    let cases: &[(Vec<f32>, u32, f32, f32, &str)] = &[
        (
            sine(2048, 80.0, 48_000.0, 0.3),
            48_000,
            80.0,
            5.0,
            "a very low voice",
        ),
        (
            sine(2048, 120.0, 48_000.0, 0.3),
            48_000,
            120.0,
            6.0,
            "a low voice",
        ),
        (
            sine(2048, 200.0, 48_000.0, 0.3),
            48_000,
            200.0,
            10.0,
            "an average voice",
        ),
        (
            sine(2048, 400.0, 48_000.0, 0.3),
            48_000,
            400.0,
            20.0,
            "a high voice",
        ),
        (
            sine(2048, 120.0, 16_000.0, 0.3),
            16_000,
            120.0,
            6.0,
            "a low voice at 16 kHz",
        ),
        (
            sine(2048, 200.0, 16_000.0, 0.3),
            16_000,
            200.0,
            10.0,
            "an average voice at 16 kHz",
        ),
    ];
    for (audio, sample_rate, want, tolerance, what) in cases {
        near(detect_pitch(audio, *sample_rate), *want, *tolerance, what);
    }
}

#[test]
fn pitch_should_never_be_reported_an_octave_too_low() {
    for hz in [150.0f32, 220.0, 250.0, 280.0, 300.0, 330.0, 350.0] {
        for sample_rate in [48_000u32, 16_000] {
            let audio = sine(2048, hz, sample_rate as f32, 0.3);
            let got = detect_pitch(&audio, sample_rate);
            assert!(
                (got - hz).abs() / hz < 0.06,
                "{hz} Hz at {sample_rate} Hz was reported as {got} Hz"
            );
        }
    }
}

// ---- writing the audio out --------------------------------------------

#[test]
fn wav_files_have_a_correct_header() {
    let wav = to_wav_bytes(&[0.0; 10], 16_000, 1);
    assert_eq!(wav.len(), 44 + 20, "44-byte header plus 2 bytes per sample");
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 20);
    assert_eq!(
        u16::from_le_bytes(wav[22..24].try_into().unwrap()),
        1,
        "channels"
    );
    assert_eq!(
        u32::from_le_bytes(wav[24..28].try_into().unwrap()),
        16_000,
        "sample rate"
    );
    assert_eq!(
        u16::from_le_bytes(wav[34..36].try_into().unwrap()),
        16,
        "bits per sample"
    );
    assert_eq!(
        u32::from_le_bytes(wav[40..44].try_into().unwrap()),
        20,
        "data size"
    );

    // An empty recording still produces a valid, empty file.
    assert_eq!(to_wav_bytes(&[], 16_000, 1).len(), 44);
}

#[test]
fn samples_outside_the_allowed_range_are_pulled_back_in() {
    // (sample, expected 16-bit value, why)
    let cases: &[(f32, i16, &str)] = &[
        (0.0, 0, "silence"),
        (1.0, i16::MAX, "the loudest allowed value"),
        (-1.0, -i16::MAX, "the quietest allowed value"),
        (
            2.0,
            i16::MAX,
            "past the top: pulled back, not wrapped around",
        ),
        (
            -2.0,
            -i16::MAX,
            "past the bottom: pulled back, not wrapped around",
        ),
    ];
    for &(sample, want, what) in cases {
        let wav = to_wav_bytes(&[sample], 16_000, 1);
        let got = i16::from_le_bytes(wav[44..46].try_into().unwrap());
        assert_eq!(got, want, "{what}");
    }
}

// ---- settings that have to survive a restart --------------------------

#[test]
fn tray_settings_survive_a_restart() {
    // (stored text, expected debug line, expected key, why)
    let cases: &[(&str, bool, &str, &str)] = &[
        (
            r#"{"debug_stats":true,"shortcut":"CommandOrControl+Shift+D"}"#,
            true,
            "CommandOrControl+Shift+D",
            "both saved",
        ),
        (
            r#"{"debug_stats":false}"#,
            false,
            "F3",
            "a file from before the key could be changed",
        ),
        (
            r#"{"language":"en","debug_stats":true}"#,
            true,
            "F3",
            "a file from when the language menu existed still loads",
        ),
        (
            r#"{}"#,
            false,
            "F3",
            "an empty settings file loads as defaults",
        ),
    ];
    for &(stored, want_debug, want_key, what) in cases {
        let prefs: Prefs = serde_json::from_str(stored).expect(what);
        assert_eq!(prefs.debug_stats, want_debug, "{what}: debug line");
        assert_eq!(prefs.shortcut, want_key, "{what}: key");
    }

    let text = serde_json::to_string(&Prefs {
        debug_stats: true,
        shortcut: "Alt+Space".to_string(),
        ..Prefs::default()
    })
    .unwrap();
    let loaded: Prefs = serde_json::from_str(&text).unwrap();
    assert!(loaded.debug_stats, "written out and read back");
    assert_eq!(loaded.shortcut, "Alt+Space");

    // A damaged file is rejected rather than half-read, so the caller can
    // fall back to the defaults.
    assert!(serde_json::from_str::<Prefs>("not json at all").is_err());
}

// ---- the settings that used to live in the hidden window ---------------

#[test]
fn a_settings_file_from_before_the_move_keeps_its_defaults() {
    // Only the two settings that were ever written by the old app.
    let old = r#"{"debug_stats":true,"shortcut":"Alt+Space"}"#;
    let prefs: Prefs = serde_json::from_str(old).expect("an old file still loads");

    assert!(prefs.debug_stats);
    assert_eq!(prefs.shortcut, "Alt+Space");
    assert!(prefs.active_local_model_id.is_none());
    assert!(
        !prefs.migrated_from_browser,
        "the browser settings have not been copied over yet"
    );
}

#[test]
fn every_setting_survives_being_written_and_read_back() {
    let saved = Prefs {
        debug_stats: true,
        shortcut: "CommandOrControl+Shift+D".to_string(),
        active_local_model_id: Some("whisper-small".to_string()),
        selected_microphone: Some("MacBook Pro Microphone".to_string()),
        migrated_from_browser: true,
    };
    let text = serde_json::to_string(&saved).unwrap();
    let loaded: Prefs = serde_json::from_str(&text).unwrap();

    assert_eq!(loaded.shortcut, saved.shortcut);
    assert_eq!(
        loaded.active_local_model_id.as_deref(),
        Some("whisper-small")
    );
    assert_eq!(
        loaded.selected_microphone.as_deref(),
        Some("MacBook Pro Microphone")
    );
    assert!(loaded.migrated_from_browser);
}

#[test]
fn a_settings_file_with_the_old_microphone_number_still_loads() {
    // The microphone used to be saved as a WirePlumber number, which meant
    // nothing on a Mac. The name replaced it; the old number is ignored
    // rather than making the whole file unreadable.
    let prefs: Prefs = serde_json::from_str(
        r#"{"selected_audio_device_id":57,"active_local_model_id":"whisper-turbo"}"#,
    )
    .expect("an old file still loads");
    assert_eq!(prefs.selected_microphone, None, "back to the system's own");
    assert_eq!(
        prefs.active_local_model_id.as_deref(),
        Some("whisper-turbo"),
        "and everything beside it survives"
    );
}

// ---- copying the settings out of the browser, once ---------------------

// Browser storage keeps everything as text, so every value arrives as text.
fn browser(pairs: &[(&str, &str)]) -> BrowserSettings {
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    BrowserSettings {
        active_local_model_id: get("active_local_model_id"),
    }
}

#[test]
fn settings_saved_in_the_browser_are_carried_over() {
    let mut prefs = Prefs::default();
    let copied = apply_browser_settings(
        &mut prefs,
        browser(&[("active_local_model_id", "parakeet-v3-int8")]),
    );

    assert!(copied);
    assert_eq!(
        prefs.active_local_model_id.as_deref(),
        Some("parakeet-v3-int8")
    );
    assert!(prefs.migrated_from_browser, "and it is marked as done");
}

#[test]
fn the_copy_happens_once_and_never_undoes_a_later_change() {
    let mut prefs = Prefs::default();
    apply_browser_settings(
        &mut prefs,
        browser(&[("active_local_model_id", "whisper-small")]),
    );
    assert_eq!(
        prefs.active_local_model_id.as_deref(),
        Some("whisper-small")
    );

    // The user then picks a different model in Settings.
    prefs.active_local_model_id = Some("whisper-turbo".to_string());

    // The browser still holds the old value, but it must not win again.
    let copied = apply_browser_settings(
        &mut prefs,
        browser(&[("active_local_model_id", "whisper-small")]),
    );
    assert!(!copied, "the second attempt does nothing");
    assert_eq!(
        prefs.active_local_model_id.as_deref(),
        Some("whisper-turbo")
    );
}

#[test]
fn empty_browser_values_are_left_behind() {
    let mut prefs = Prefs {
        active_local_model_id: Some("already-here".to_string()),
        ..Prefs::default()
    };
    apply_browser_settings(&mut prefs, browser(&[("active_local_model_id", "")]));

    assert_eq!(
        prefs.active_local_model_id.as_deref(),
        Some("already-here"),
        "a blank name must not wipe a real one"
    );
}

// ---- the two chimes ---------------------------------------------------

#[test]
fn a_chime_starts_and_ends_in_silence() {
    // A note that jumps straight to full loudness clicks. These are the
    // moments that must be silent for it not to.
    for notes in [&START_CHIME, &DONE_CHIME] {
        near(
            chime_at(notes, 0.0),
            0.0,
            0.0001,
            "silent at the very start",
        );
        let after = chime_seconds(notes) + 0.05;
        near(
            chime_at(notes, after),
            0.0,
            0.0001,
            "silent once it is over",
        );
        near(
            chime_at(notes, -0.1),
            0.0,
            0.0001,
            "silent before it begins",
        );
    }
}

#[test]
fn a_chime_never_gets_loud_enough_to_distort() {
    // Both notes overlap, so their loudness adds up. Anything past 1.0
    // would be clipped by the speakers into a buzz.
    for notes in [&START_CHIME, &DONE_CHIME] {
        let steps = 20_000;
        let length = chime_seconds(notes);
        let mut loudest = 0.0f32;
        for step in 0..steps {
            let value = chime_at(notes, length * step as f32 / steps as f32);
            loudest = loudest.max(value.abs());
        }
        assert!(loudest > 0.05, "it has to be audible: got {loudest}");
        assert!(loudest < 1.0, "it must not distort: got {loudest}");
    }
}

#[test]
fn one_note_fades_in_quickly_and_out_slowly() {
    // (how far into the note, what the loudness should be, why)
    let cases: &[(f32, f32, &str)] = &[
        (-0.01, 0.0, "before it starts"),
        (0.0, 0.0, "silent at the moment it starts"),
        (0.015, 0.06, "half way through the fade in"),
        (0.03, 0.12, "at its loudest once faded in"),
        (0.215, 0.06, "half way through the fade out"),
        (0.4, 0.0, "silent at the moment it ends"),
        (0.5, 0.0, "after it has ended"),
    ];
    for &(age, want, what) in cases {
        near(note_loudness(age, 0.4), want, 0.005, what);
    }
}

#[test]
fn a_chime_lasts_until_its_last_note_has_finished() {
    // The second note starts late, so the first one ending is not the end.
    near(
        chime_seconds(&START_CHIME),
        0.5,
        0.0001,
        "0.08s in plus 0.42s long",
    );
    near(
        chime_seconds(&DONE_CHIME),
        0.6,
        0.0001,
        "0.08s in plus 0.52s",
    );
}

// ---- deleting recordings ----------------------------------------------

#[test]
fn deleting_recordings_removes_only_recordings() {
    let dir = std::env::temp_dir().join(format!("omegawhisper-delete-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // (file name, should it survive, why)
    let files: &[(&str, bool, &str)] = &[
        ("2026-01-01.wav", false, "a recording"),
        ("2026-01-01-model-input.wav", false, "what the model heard"),
        ("notes.txt", true, "someone else's file"),
        ("tray-prefs.json", true, "a settings file"),
        ("recording.wav.bak", true, "a backup, not a recording"),
        ("WAV", true, "no extension at all"),
    ];
    for (name, _, _) in files {
        fs::write(dir.join(name), b"x").unwrap();
    }
    // A folder must survive too - only files are considered.
    fs::create_dir(dir.join("old")).unwrap();
    fs::write(dir.join("old").join("kept.wav"), b"x").unwrap();

    let deleted = delete_recordings_in(&dir).unwrap();
    assert_eq!(deleted, 2, "only the two recordings should go");

    for (name, survives, why) in files {
        assert_eq!(dir.join(name).exists(), *survives, "{name}: {why}");
    }
    assert!(
        dir.join("old").join("kept.wav").exists(),
        "subfolders untouched"
    );
    assert!(dir.exists(), "the folder itself must stay");

    // Running it again on an empty folder is not an error.
    assert_eq!(delete_recordings_in(&dir).unwrap(), 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn deleting_from_a_missing_folder_is_an_error_not_a_panic() {
    let missing = std::env::temp_dir().join("omegawhisper-does-not-exist-at-all");
    let _ = fs::remove_dir_all(&missing);
    assert!(delete_recordings_in(&missing).is_err());
}
