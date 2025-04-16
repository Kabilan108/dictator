import { useEffect, useRef } from "react";

import { KEY_HOLD_DELAY } from "@/config";
import type { RecordingState } from "@/types";

interface KeyboardControlsProps {
  state: RecordingState;
  showSettings: boolean;
  onStart: () => Promise<void> | void;
  onStop: () => Promise<void> | void;
  onReset: () => void;
}

export default function useKeyboardControls({
  state,
  showSettings,
  onStart,
  onStop,
  onReset
}: KeyboardControlsProps): void {
  const keyDownTimestampRef = useRef<number>(0);
  const holdTimerRef = useRef<NodeJS.Timeout | null>(null);
  const recordingModeRef = useRef<"tap" | "hold" | null>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (showSettings) return;

      if (e.code === "Space" && !e.repeat) {
        e.preventDefault();
        if (state === "idle") {
          keyDownTimestampRef.current = Date.now();
          onStart();
          recordingModeRef.current = "tap";
          holdTimerRef.current = setTimeout(() => {
            recordingModeRef.current = "hold";
          }, KEY_HOLD_DELAY);
        } else if (state === "recording" && recordingModeRef.current === "tap") {
          if (holdTimerRef.current) {
            clearTimeout(holdTimerRef.current);
            holdTimerRef.current = null;
          }
          onStop();
        }
      } else if (e.code === "Escape") {
        e.preventDefault();
        if (!showSettings && state === "results") {
          onReset();
        }
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      if (showSettings) return;

      if (e.code === "Space" && state === "recording") {
        e.preventDefault();
        if (holdTimerRef.current) {
          clearTimeout(holdTimerRef.current);
          holdTimerRef.current = null;
        }
        if (recordingModeRef.current === "hold") {
          onStop();
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
      if (holdTimerRef.current) clearTimeout(holdTimerRef.current)
    };
  }, [state, showSettings, onStart, onStop, onReset]);
}
