import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";

export interface MeetingUiState {
  active: boolean;
  streaming: boolean;
  captionsVisible: boolean;
  /** Wall-clock ms when the session started (for the ticking timer). */
  startedAtMs: number;
}

const IDLE: MeetingUiState = {
  active: false,
  streaming: false,
  captionsVisible: false,
  startedAtMs: 0,
};

/**
 * Mirrors the backend meeting session into the UI: fetched on mount (the
 * window can open mid-meeting) and kept fresh via meeting-started/ended.
 */
export function useMeetingState(): MeetingUiState {
  const [state, setState] = useState<MeetingUiState>(IDLE);

  const refresh = useCallback(async () => {
    const result = await commands.getMeetingState();
    if (result.status !== "ok") return;
    const s = result.data;
    setState(
      s.active
        ? {
            active: true,
            streaming: s.streaming,
            captionsVisible: s.captions_visible,
            startedAtMs: Date.now() - s.elapsed_secs * 1000,
          }
        : IDLE,
    );
  }, []);

  useEffect(() => {
    refresh();
    const unlistenStart = listen("meeting-started", () => refresh());
    const unlistenEnd = listen("meeting-ended", () =>
      setState((prev) => ({ ...prev, active: false })),
    );
    return () => {
      unlistenStart.then((fn) => fn());
      unlistenEnd.then((fn) => fn());
    };
  }, [refresh]);

  return state;
}

export function formatElapsed(startedAtMs: number, nowMs: number): string {
  const secs = Math.max(0, Math.floor((nowMs - startedAtMs) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) {
    return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  }
  return `${m}:${String(s).padStart(2, "0")}`;
}
