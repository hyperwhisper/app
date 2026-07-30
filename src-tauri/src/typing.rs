// Putting the finished text into whatever app has focus.

use std::thread;
use std::time::Duration;

// How many UTF-16 units to put in one key event. A Unicode key event carries
// only a short string, so long text is sent in several events.
#[cfg(target_os = "macos")]
pub(crate) const CHUNK_UTF16_UNITS: usize = 20;

// Whether this app is allowed to control the computer (Accessibility).
// Without it the key events are created but the system drops them, so the
// text silently never appears.
#[cfg(target_os = "macos")]
pub(crate) fn accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

// The return separates the macOS path from the Linux one below it.
#[allow(clippy::needless_return)]
pub(crate) fn type_text_internal(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Post the text straight to the window server as Unicode key events.
        //
        // This replaces `osascript ... keystroke`, which spawned a process per
        // transcription, needed the Automation permission on top of
        // Accessibility, and typed through the current keyboard layout - so
        // any character the layout cannot produce (Cyrillic on a US layout)
        // came out wrong. Unicode key events do not use the layout.
        use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        if !accessibility_granted() {
            return Err(
                "Accessibility permission is not granted, so text cannot be typed. \
                 Add Omegawhisper in System Settings > Privacy & Security > Accessibility."
                    .to_string(),
            );
        }

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "Failed to create a keyboard event source".to_string())?;

        // The shortcut that stops recording is F3, a function key. Give the
        // physical key time to be released and the target window time to take
        // focus back, otherwise the first characters land nowhere.
        thread::sleep(Duration::from_millis(120));

        // One event carries only a short Unicode string, so send the text in
        // small pieces. Split on character boundaries, never inside a
        // surrogate pair, or the character is corrupted.
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut start = 0;
        while start < utf16.len() {
            let mut end = std::cmp::min(start + CHUNK_UTF16_UNITS, utf16.len());
            // A leading surrogate at the end means the pair is split - keep it
            // with its trailing half in the next chunk.
            if end < utf16.len() && (0xD800..0xDC00).contains(&utf16[end - 1]) {
                end -= 1;
            }
            let chunk = String::from_utf16_lossy(&utf16[start..end]);

            for key_down in [true, false] {
                let event = CGEvent::new_keyboard_event(source.clone(), 0, key_down)
                    .map_err(|_| "Failed to create a keyboard event".to_string())?;
                // Events built from the live hardware state inherit whatever
                // modifiers are held right now. F3 sets the Fn modifier, and a
                // character carrying Fn (or Command) is read as a shortcut and
                // thrown away instead of typed. Send plain characters only.
                event.set_flags(CGEventFlags::CGEventFlagNull);
                event.set_string(&chunk);
                event.post(CGEventTapLocation::HID);
            }

            // Electron apps (Teams, VS Code) drop characters without a pause.
            thread::sleep(Duration::from_millis(2));
            start = end;
        }

        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Try ydotool first (works on both Wayland and X11 via uinput)
        // Use --key-delay 0 for fastest typing
        let ydotool_result = std::process::Command::new("ydotool")
            .args(["type", "--key-delay=0", "--", text])
            .status();

        if let Ok(status) = ydotool_result {
            if status.success() {
                return Ok(());
            }
        }

        // Try wtype (Wayland - requires compositor support)
        let wtype_result = std::process::Command::new("wtype").arg(text).status();

        if let Ok(status) = wtype_result {
            if status.success() {
                return Ok(());
            }
        }

        // Fall back to xdotool (X11)
        let xdotool_result = std::process::Command::new("xdotool")
            .args(["type", "--clearmodifiers", text])
            .status();

        if let Ok(status) = xdotool_result {
            if status.success() {
                return Ok(());
            }
        }

        Err("Failed to type text: ydotool, wtype, and xdotool all failed".to_string())
    }
}
