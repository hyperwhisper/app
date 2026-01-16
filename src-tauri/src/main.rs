// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{Parser, Subcommand};

/// HyperWhisper - The most productive speech to text app for Linux
#[derive(Parser, Debug)]
#[command(name = "hyperwhisper")]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage transcription
    Transcribe {
        #[command(subcommand)]
        action: TranscribeAction,
    },
}

#[derive(Subcommand, Debug)]
enum TranscribeAction {
    /// Toggle transcription on/off (requires running HyperWhisper instance)
    Toggle,
}

fn main() {
    // Parse CLI arguments - clap handles --version and --help automatically
    let args = Args::parse();

    match args.command {
        Some(Commands::Transcribe { action }) => match action {
            TranscribeAction::Toggle => {
                if let Err(e) = toggle_recording() {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        },
        None => {
            // Run the Tauri application
            hyperwhisper_lib::run()
        }
    }
}

/// Toggle recording via D-Bus
fn toggle_recording() -> Result<(), Box<dyn std::error::Error>> {
    // Use blocking D-Bus call since we're in a simple CLI context
    let connection = zbus::blocking::Connection::session()?;

    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "dev.hyperwhisper",
        "/dev/hyperwhisper",
        "dev.hyperwhisper",
    )?;

    let _result: bool = proxy.call("ToggleRecording", &())?;

    Ok(())
}
