// The folder the app keeps its recordings, models and settings in.

use chrono::Local;
use std::fs;
use std::path::PathBuf;

// Get the recordings directory, creating it if necessary
pub(crate) fn get_recordings_dir() -> Result<PathBuf, String> {
    let data_dir =
        dirs::data_local_dir().ok_or_else(|| "Could not find local data directory".to_string())?;
    let recordings_dir = data_dir.join("omegawhisper").join("recordings");

    if !recordings_dir.exists() {
        fs::create_dir_all(&recordings_dir)
            .map_err(|e| format!("Failed to create recordings directory: {}", e))?;
    }

    Ok(recordings_dir)
}

// The data folder was called "hyperwhisper" before the app was renamed to
// Omegawhisper. Move it to the new name once, so the downloaded models
// (several GB) and old recordings are kept instead of downloaded again.
// Must run before anything else touches the data folder.
pub(crate) fn migrate_legacy_data_dir() {
    let data_dir = match dirs::data_local_dir() {
        Some(d) => d,
        None => return,
    };
    let old_dir = data_dir.join("hyperwhisper");
    let new_dir = data_dir.join("omegawhisper");

    if !old_dir.is_dir() {
        return;
    }

    if let Ok(mut entries) = fs::read_dir(&new_dir) {
        if entries.next().is_some() {
            // Both folders hold data - do not touch either one.
            eprintln!(
                "Data folder migration skipped: {} already has files. Old data is still in {}",
                new_dir.display(),
                old_dir.display()
            );
            return;
        }
        // New folder exists but is empty: remove it so the rename can use the name.
        if let Err(e) = fs::remove_dir(&new_dir) {
            eprintln!("Could not remove empty {}: {}", new_dir.display(), e);
            return;
        }
    }

    match fs::rename(&old_dir, &new_dir) {
        Ok(()) => eprintln!("Moved {} to {}", old_dir.display(), new_dir.display()),
        Err(e) => eprintln!(
            "Failed to move {} to {}: {}",
            old_dir.display(),
            new_dir.display(),
            e
        ),
    }
}

// Deletes every recording in a folder. Only .wav files, so anything else that
// happens to be in there survives. Returns how many went.
pub(crate) fn delete_recordings_in(dir: &std::path::Path) -> Result<usize, String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Could not read {}: {}", dir.display(), e))?;
    let mut deleted = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("wav") {
            match fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) => eprintln!("Could not delete {}: {}", path.display(), e),
            }
        }
    }
    Ok(deleted)
}

// Everything the app prints goes to one file, whatever started it - Finder,
// the tray, or a terminal. Launched from Finder there is no terminal to print
// to, so a dictation that went wrong used to leave no trace at all.
#[cfg(unix)]
pub(crate) fn redirect_output_to_log() {
    let Some(dir) = dirs::data_local_dir() else {
        return;
    };
    let path = dir.join("omegawhisper").join("omegawhisper.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Start over once the file gets big rather than growing without end.
    if fs::metadata(&path)
        .map(|m| m.len() > 5_000_000)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(&path);
    }

    let Ok(file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
    // The file must outlive this function: the two descriptors above now
    // point at it and closing it here would close them too.
    std::mem::forget(file);

    eprintln!(
        "\n===== started {} =====",
        Local::now().format("%F %H:%M:%S")
    );
}
