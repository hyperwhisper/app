import { invoke } from "@tauri-apps/api/core";

// The settings the app used to keep in the browser's own storage, which only
// exists while a window does. Rust cannot read that storage, so a window has to
// hand the values over. Rust does the copying once and then ignores us.
export const BROWSER_SETTING_KEYS = ["active_local_model_id"] as const;

// Returns true the one time the settings were actually copied across.
export async function handOverBrowserSettings(): Promise<boolean> {
  const values: Record<string, string | null> = {};
  for (const key of BROWSER_SETTING_KEYS) {
    values[key] = localStorage.getItem(key);
  }
  return await invoke<boolean>("migrate_browser_settings", { values });
}
