import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

// LocalStorage key - single storage location for all API keys (trial or user-provided)
const API_KEY_STORAGE = "hyperwhisper_api_key";

// Trial key types
// Note: duration fields are floats from the server
export interface TrialProvisionResponse {
  key?: string;
  key_prefix: string;
  remaining_duration_seconds: number;  // float
  remaining_sessions: number;          // int
  max_session_duration_seconds: number; // float
  expires_at: string;
  quota_exceeded: boolean;
  expired: boolean;
}

export interface TrialStatusResponse {
  active: boolean;
  remaining_duration_seconds: number;  // float
  remaining_sessions: number;          // int
  expires_at: string;
  expired: boolean;
  quota_exceeded: boolean;
  upgrade_url?: string;
}

export interface TrialUsageResponse {
  total_duration_seconds: number;      // float
  total_sessions: number;              // int
  remaining_duration_seconds: number;  // float
  remaining_sessions: number;          // int
  max_duration_seconds: number;        // float
  max_sessions: number;                // int
  max_session_duration_seconds: number; // float
  quota_exceeded: boolean;
}

export interface ServerSettingsResponse {
  provisioned_key: string | null;
  trial_info: TrialProvisionResponse | null;
  error: string | null;
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

  // Get the stored API key (could be trial or user-provided)
  const getStoredApiKey = useCallback((): string | null => {
    const key = localStorage.getItem(API_KEY_STORAGE);
    return key?.trim() || null;
  }, []);

  // Check if the stored key is a trial key
  const isTrialKey = useCallback((key: string | null): boolean => {
    return key?.startsWith("hw_trial_") ?? false;
  }, []);

  // Store the API key
  const storeApiKey = useCallback((key: string) => {
    localStorage.setItem(API_KEY_STORAGE, key);
  }, []);

  // Check status of an existing key
  const checkKeyStatus = useCallback(async (key: string): Promise<TrialStatusResponse> => {
    return await invoke<TrialStatusResponse>("get_trial_status", { apiKey: key });
  }, []);

  // Get usage statistics
  const getUsage = useCallback(async (key: string): Promise<TrialUsageResponse> => {
    return await invoke<TrialUsageResponse>("get_trial_usage", { apiKey: key });
  }, []);

  // Initialize trial key on app start
  const initialize = useCallback(async () => {
    setIsInitializing(true);
    setState({ status: "loading" });

    try {
      const useHyperwhisperServer = localStorage.getItem("use_hyperwhisper_server") !== "false";
      const serverUrl = localStorage.getItem("hyperwhisper_server_url") || "hyperwhisper.dev";
      const useHttps = localStorage.getItem("hyperwhisper_server_https") !== "false";

      // Get the stored API key (could be trial or user-provided)
      const existingKey = getStoredApiKey();
      const keyIsTrialKey = isTrialKey(existingKey);

      // Call backend - it will auto-provision if needed (when existingKey is null)
      console.log("[useTrialKey] Calling set_hyperwhisper_server_settings with:", {
        useHyperwhisperServer,
        serverUrl: serverUrl.trim() || "hyperwhisper.dev",
        useHttps,
        apiKey: existingKey ? "[REDACTED]" : null,
      });

      const response = await invoke<ServerSettingsResponse>("set_hyperwhisper_server_settings", {
        useHyperwhisperServer,
        serverUrl: serverUrl.trim() || "hyperwhisper.dev",
        useHttps,
        apiKey: existingKey,
      });

      console.log("[useTrialKey] Response:", {
        provisioned_key: response.provisioned_key ? "[REDACTED]" : null,
        trial_info: response.trial_info,
        error: response.error,
      });

      // If user has their own (non-trial) API key, we're done
      if (existingKey && !keyIsTrialKey) {
        setState({ status: "has_api_key" });
        setIsInitializing(false);
        return;
      }

      // Check if a key was provisioned
      if (response.provisioned_key) {
        console.log("[useTrialKey] Storing provisioned key...");
        storeApiKey(response.provisioned_key);

        if (response.trial_info) {
          if (response.trial_info.quota_exceeded) {
            setState({
              status: "quota_exceeded",
              key: response.provisioned_key,
              info: {
                active: false,
                remaining_duration_seconds: response.trial_info.remaining_duration_seconds,
                remaining_sessions: response.trial_info.remaining_sessions,
                expires_at: response.trial_info.expires_at,
                expired: response.trial_info.expired,
                quota_exceeded: response.trial_info.quota_exceeded,
              },
            });
          } else if (response.trial_info.expired) {
            setState({
              status: "expired",
              key: response.provisioned_key,
              info: {
                active: false,
                remaining_duration_seconds: response.trial_info.remaining_duration_seconds,
                remaining_sessions: response.trial_info.remaining_sessions,
                expires_at: response.trial_info.expires_at,
                expired: response.trial_info.expired,
                quota_exceeded: response.trial_info.quota_exceeded,
              },
            });
          } else {
            setState({
              status: "active",
              key: response.provisioned_key,
              info: {
                active: true,
                remaining_duration_seconds: response.trial_info.remaining_duration_seconds,
                remaining_sessions: response.trial_info.remaining_sessions,
                expires_at: response.trial_info.expires_at,
                expired: response.trial_info.expired,
                quota_exceeded: response.trial_info.quota_exceeded,
              },
            });
          }
        }
        setIsInitializing(false);
        return;
      }

      // Check if there was an error during provisioning
      if (response.error) {
        setState({ status: "error", error: response.error });
        setIsInitializing(false);
        return;
      }

      // No key provisioned and no error - we passed an existing trial key, check its status
      if (existingKey && keyIsTrialKey) {
        try {
          const status = await checkKeyStatus(existingKey);
          if (status.quota_exceeded) {
            setState({ status: "quota_exceeded", key: existingKey, info: status });
          } else if (status.expired) {
            setState({ status: "expired", key: existingKey, info: status });
          } else if (status.active) {
            setState({ status: "active", key: existingKey, info: status });
          } else {
            setState({ status: "no_key" });
          }
        } catch {
          setState({ status: "no_key" });
        }
      } else {
        setState({ status: "no_key" });
      }
    } catch (err) {
      console.error("Trial key initialization failed:", err);
      setState({ status: "error", error: String(err) });
    } finally {
      setIsInitializing(false);
    }
  }, [getStoredApiKey, isTrialKey, storeApiKey, checkKeyStatus]);

  // Refresh the trial status
  const refresh = useCallback(async () => {
    const existingKey = getStoredApiKey();

    // If user has their own (non-trial) API key, update state
    if (existingKey && !isTrialKey(existingKey)) {
      setState({ status: "has_api_key" });
      return;
    }

    // If no key at all, re-initialize to attempt provisioning
    if (!existingKey) {
      initialize();
      return;
    }

    // existingKey is a trial key - check its status
    try {
      const status = await checkKeyStatus(existingKey);
      if (status.quota_exceeded) {
        setState({ status: "quota_exceeded", key: existingKey, info: status });
      } else if (status.expired) {
        setState({ status: "expired", key: existingKey, info: status });
      } else if (status.active) {
        setState({ status: "active", key: existingKey, info: status });
      }
    } catch (err) {
      console.error("Failed to refresh trial status:", err);
    }
  }, [getStoredApiKey, isTrialKey, checkKeyStatus, initialize]);

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
