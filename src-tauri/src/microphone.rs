// Which microphone to record from, and what format it will give us.

use crate::AudioState;
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use tauri::State;

/// One microphone the app could record from.
#[derive(Clone, serde::Serialize)]
pub struct AudioDevice {
    pub name: String,
    /// True for the one the system would use if nothing were chosen.
    pub is_default: bool,
}

// Every microphone the system will let the app open. Asked of cpal, which is
// already how the audio is recorded, so this list can never disagree with what
// the recording actually gets. It used to come from `wpctl`, a Linux-only
// program that is not installed on a Mac, so the list was always empty here.
#[tauri::command]
pub(crate) fn list_audio_devices() -> Result<Vec<AudioDevice>, String> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    let devices = host
        .input_devices()
        .map_err(|e| format!("Could not ask the system for microphones: {}", e))?;

    Ok(devices
        .filter_map(|device| device.name().ok())
        .map(|name| AudioDevice {
            is_default: Some(&name) == default_name.as_ref(),
            name,
        })
        .collect())
}

#[tauri::command]
pub(crate) fn set_selected_device(state: State<'_, AudioState>, name: Option<String>) {
    let name = name.filter(|n| !n.trim().is_empty());
    state.update_prefs(|p| p.selected_microphone = name);
}

// The microphone to record from: the one chosen in Settings, or the system's
// own choice when nothing is chosen or the chosen one has been unplugged.
pub(crate) fn get_input_device(wanted: Option<&str>) -> Result<Device, String> {
    let host = cpal::default_host();

    if let Some(wanted) = wanted {
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if device.name().is_ok_and(|name| name == wanted) {
                    eprintln!("microphone: {}", wanted);
                    return Ok(device);
                }
            }
        }
        // Said out loud rather than swapped silently: a recording that comes
        // back from the wrong microphone is otherwise impossible to explain.
        eprintln!(
            "microphone: \"{}\" is not connected, using the system's own choice instead",
            wanted
        );
    }

    // Linux: "pipewire" follows whatever WirePlumber has set as default and
    // handles Bluetooth better than the raw ALSA devices behind it.
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if device.name().is_ok_and(|name| name == "pipewire") {
                eprintln!("microphone: pipewire (follows the system default)");
                return Ok(device);
            }
        }
    }

    let device = host
        .default_input_device()
        .ok_or_else(|| "No microphone found. Check System Settings > Sound > Input.".to_string())?;
    if let Ok(name) = device.name() {
        eprintln!("microphone: {} (the system's own choice)", name);
    }
    Ok(device)
}

// Get a safe stream config that works with Bluetooth devices
// Bluetooth audio on Linux (especially with PipeWire) can crash GNOME when using
// certain buffer sizes or sample rates. This function tries to find a safer config.
pub(crate) fn get_safe_input_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    // First, try to get supported configs and find one that's known to work well
    if let Ok(configs) = device.supported_input_configs() {
        let configs: Vec<_> = configs.collect();

        // Prefer 48000 Hz or 44100 Hz with F32 format - these are most compatible
        let preferred_rates = [48000u32, 44100, 16000, 32000, 96000];

        for rate in preferred_rates {
            for config in &configs {
                if config.min_sample_rate().0 <= rate
                    && config.max_sample_rate().0 >= rate
                    && config.sample_format() == SampleFormat::F32
                {
                    return Ok((*config).with_sample_rate(cpal::SampleRate(rate)));
                }
            }
            // If F32 not available at this rate, try I16
            for config in &configs {
                if config.min_sample_rate().0 <= rate
                    && config.max_sample_rate().0 >= rate
                    && config.sample_format() == SampleFormat::I16
                {
                    return Ok((*config).with_sample_rate(cpal::SampleRate(rate)));
                }
            }
        }
    }

    // Fall back to default config if no preferred config found
    device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {}", e))
}
