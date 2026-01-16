import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

// LocalStorage keys
const TRIAL_KEY_STORAGE = "hyperwhisper_trial_key";
const API_KEY_STORAGE = "hyperwhisper_api_key";

// Trial key types
export interface TrialProvisionResponse {
  key?: string;
  key_prefix: string;
  remaining_duration_seconds: number;
  remaining_sessions: number;
  max_session_duration_seconds: number;
  expires_at: string;
  quota_exceeded: boolean;
  expired: boolean;
}

export interface TrialStatusResponse {
  active: boolean;
  remaining_duration_seconds: number;
  remaining_sessions: number;
  expires_at: string;
  expired: boolean;
  quota_exceeded: boolean;
  upgrade_url?: string;
}

export interface TrialUsageResponse {
  total_duration_seconds: number;
  total_sessions: number;
  remaining_duration_seconds: number;
  remaining_sessions: number;
  max_duration_seconds: number;
  max_sessions: number;
  max_session_duration_seconds: number;
  quota_exceeded: boolean;
}

export type TrialKeyState =
  | { status: "loading" }
  | { status: "has_api_key" } // User has their own API key, no trial needed
  | { status: "no_key" }
  | { status: "active"; key: string; info: TrialStatusResponse }
  | { status: "expired"; key: string; info: TrialStatusResponse }
  | { status: "quota_exceeded"; key: string; info: TrialStatusResponse }
  | { status: "error"; error: string };

export function useTrialKey() {
  const [state, setState] = useState<TrialKeyState>({ status: "loading" });
  const [isInitializing, setIsInitializing] = useState(true);

  // Check if user has a non-trial API key
  const hasUserApiKey = useCallback((): boolean => {
    const apiKey = localStorage.getItem(API_KEY_STORAGE) || "";
    // User has their own key if it's set and NOT a trial key
    return apiKey.trim() !== "" && !apiKey.startsWith("hw_trial_");
  }, []);

  // Get the stored trial key
  const getStoredTrialKey = useCallback((): string | null => {
    return localStorage.getItem(TRIAL_KEY_STORAGE);
  }, []);

  // Store the trial key
  const storeTrialKey = useCallback((key: string) => {
    localStorage.setItem(TRIAL_KEY_STORAGE, key);
    // Also update the hyperwhisper_api_key for recording to work
    localStorage.setItem(API_KEY_STORAGE, key);
  }, []);

  // Clear the stored trial key
  const clearTrialKey = useCallback(() => {
    localStorage.removeItem(TRIAL_KEY_STORAGE);
  }, []);

  // Sync server settings to backend before making API calls
  const syncServerSettings = useCallback(async () => {
    const useHyperwhisperServer = localStorage.getItem("use_hyperwhisper_server") !== "false";
    const serverUrl = localStorage.getItem("hyperwhisper_server_url") || "hyperwhisper.dev";
    const useHttps = localStorage.getItem("hyperwhisper_server_https") !== "false";
    const apiKey = localStorage.getItem(API_KEY_STORAGE) || null;

    await invoke("set_hyperwhisper_server_settings", {
      useHyperwhisperServer,
      serverUrl: serverUrl.trim() || "hyperwhisper.dev",
      useHttps,
      apiKey: apiKey?.trim() || null,
    });
  }, []);

  // Check status of an existing key
  const checkKeyStatus = useCallback(async (key: string): Promise<TrialStatusResponse> => {
    return await invoke<TrialStatusResponse>("get_trial_status", { apiKey: key });
  }, []);

  // Provision a new trial key
  const provisionKey = useCallback(async (): Promise<TrialProvisionResponse> => {
    return await invoke<TrialProvisionResponse>("provision_trial_key");
  }, []);

  // Get usage statistics
  const getUsage = useCallback(async (key: string): Promise<TrialUsageResponse> => {
    return await invoke<TrialUsageResponse>("get_trial_usage", { apiKey: key });
  }, []);

  // Provision a new key and update state
  const provisionNewKey = useCallback(async () => {
    try {
      const response = await provisionKey();

      // Only store if we got the full key (first provision)
      if (response.key) {
        storeTrialKey(response.key);

        if (response.quota_exceeded) {
          setState({
            status: "quota_exceeded",
            key: response.key,
            info: {
              active: false,
              remaining_duration_seconds: response.remaining_duration_seconds,
              remaining_sessions: response.remaining_sessions,
              expires_at: response.expires_at,
              expired: response.expired,
              quota_exceeded: response.quota_exceeded,
            },
          });
        } else if (response.expired) {
          setState({
            status: "expired",
            key: response.key,
            info: {
              active: false,
              remaining_duration_seconds: response.remaining_duration_seconds,
              remaining_sessions: response.remaining_sessions,
              expires_at: response.expires_at,
              expired: response.expired,
              quota_exceeded: response.quota_exceeded,
            },
          });
        } else {
          setState({
            status: "active",
            key: response.key,
            info: {
              active: true,
              remaining_duration_seconds: response.remaining_duration_seconds,
              remaining_sessions: response.remaining_sessions,
              expires_at: response.expires_at,
              expired: response.expired,
              quota_exceeded: response.quota_exceeded,
            },
          });
        }
      } else {
        // Device already has a trial key but we don't have it stored
        // This shouldn't normally happen, but handle it gracefully
        setState({
          status: "error",
          error: "Trial key exists for this device but was not stored. Please contact support.",
        });
      }
    } catch (err) {
      console.error("Failed to provision trial key:", err);
      setState({ status: "error", error: String(err) });
    }
  }, [provisionKey, storeTrialKey]);

  // Initialize trial key on app start
  const initialize = useCallback(async () => {
    setIsInitializing(true);
    setState({ status: "loading" });

    try {
      // First, sync server settings to backend
      await syncServerSettings();

      // Check if user has their own (non-trial) API key
      if (hasUserApiKey()) {
        setState({ status: "has_api_key" });
        setIsInitializing(false);
        return;
      }

      // Check for existing trial key
      const storedTrialKey = getStoredTrialKey();

      if (storedTrialKey) {
        // Check status of existing trial key
        try {
          const status = await checkKeyStatus(storedTrialKey);

          if (status.quota_exceeded) {
            setState({ status: "quota_exceeded", key: storedTrialKey, info: status });
          } else if (status.expired) {
            setState({ status: "expired", key: storedTrialKey, info: status });
          } else if (status.active) {
            setState({ status: "active", key: storedTrialKey, info: status });
            // Ensure hyperwhisper_api_key is synced
            localStorage.setItem(API_KEY_STORAGE, storedTrialKey);
          } else {
            // Key is not active for some other reason - try to re-provision
            clearTrialKey();
            await provisionNewKey();
          }
        } catch (err) {
          // Status check failed - could be 401/403, try re-provisioning
          console.warn("Trial status check failed, re-provisioning:", err);
          clearTrialKey();
          await provisionNewKey();
        }
      } else {
        // No stored trial key and no user API key - provision a new trial key
        await provisionNewKey();
      }
    } catch (err) {
      console.error("Trial key initialization failed:", err);
      setState({ status: "error", error: String(err) });
    } finally {
      setIsInitializing(false);
    }
  }, [syncServerSettings, hasUserApiKey, getStoredTrialKey, checkKeyStatus, clearTrialKey, provisionNewKey]);

  // Refresh the trial status
  const refresh = useCallback(async () => {
    // If user has their own API key, nothing to refresh
    if (hasUserApiKey()) {
      setState({ status: "has_api_key" });
      return;
    }

    const storedTrialKey = getStoredTrialKey();
    if (storedTrialKey) {
      try {
        const status = await checkKeyStatus(storedTrialKey);
        if (status.quota_exceeded) {
          setState({ status: "quota_exceeded", key: storedTrialKey, info: status });
        } else if (status.expired) {
          setState({ status: "expired", key: storedTrialKey, info: status });
        } else if (status.active) {
          setState({ status: "active", key: storedTrialKey, info: status });
        }
      } catch (err) {
        console.error("Failed to refresh trial status:", err);
      }
    }
  }, [hasUserApiKey, getStoredTrialKey, checkKeyStatus]);

  // Initialize on mount
  useEffect(() => {
    initialize();
  }, [initialize]);

  return {
    state,
    isInitializing,
    refresh,
    getUsage: state.status === "active" || state.status === "quota_exceeded" || state.status === "expired"
      ? () => getUsage(state.key)
      : null,
  };
}
