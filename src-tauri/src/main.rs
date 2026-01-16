// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

/// HyperWhisper - The most productive speech to text app for Linux
#[derive(Parser, Debug)]
#[command(name = "hyperwhisper")]
#[command(author, version, about, long_about = None)]
struct Args {}

fn main() {
    // Parse CLI arguments - clap handles --version and --help automatically
    let _args = Args::parse();

    // Run the Tauri application
    hyperwhisper_lib::run()
}
