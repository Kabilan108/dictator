// src/components/RecordingWindow.tsx
import { useState, useEffect, useRef, useCallback } from "react";
import { Mic, StopCircle, Copy, Settings } from "lucide-react";
import { WindowSetSize } from "@wailsjs/runtime";
import { Log } from "@/lib/utils";
import { StartRecording, StopRecording } from "@wailsjs/go/main/App";
import { useTheme } from "@/lib/ThemeContext";
import { SettingsPanel } from "./SettingsPanel";

type RecordingState = "idle" | "recording" | "transcribing" | "results";

const WINDOW_WIDTH = 300
const DEFAULT_WINDOW_HEIGHT = 180

export function RecordingWindow() {
  const { colors } = useTheme();
  const [state, setState] = useState<RecordingState>("idle");
  const [recordingTime, setRecordingTime] = useState(0);
  const [transcriptionResult, setTranscriptionResult] = useState<string>("");
  const [showSettings, setShowSettings] = useState(false);
  const timerRef = useRef<NodeJS.Timeout | null>(null);
  const finalRecordingTimeRef = useRef<number>(0);
  const keyDownTimestampRef = useRef<number>(0);
  const holdTimerRef = useRef<NodeJS.Timeout | null>(null);
  const recordingModeRef = useRef<"tap" | "hold" | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const startRecording = useCallback(async () => {
    try {
      const result = await StartRecording();
      if (!result.success) {
        throw new Error(result.error || "Unknown error starting recording");
      }
      setState("recording");
    } catch (error) {
      Log.e(`Error starting recording: ${error}`);
      setState("idle");
      alert("Failed to start recording. Please try again.");
    }
  }, []);

  const stopRecording = useCallback(async () => {
    try {
      setState("transcribing");
      const result = await StopRecording();
      if (!result.success) {
        throw new Error(result.error || "Unknown error stopping recording");
      }
      finalRecordingTimeRef.current = recordingTime;
      setTranscriptionResult(result.transcript || "");
      setState("results");
    } catch (error) {
      Log.e(`Error stopping recording: ${error}`);
      setState("idle");
      alert(`Failed to stop recording: ${error}`);
    }
  }, [recordingTime]);

  const copyToClipboard = useCallback(() => {
    navigator.clipboard.writeText(transcriptionResult).then(() => {
      Log.i("Copied transcription to clipboard");
    }).catch(err => {
      Log.e(`Failed to copy: ${err}`);
    });
  }, [transcriptionResult]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (showSettings) return; // Ignore keyboard shortcuts when settings are open

      if (e.code === "Space" && !e.repeat) {
        e.preventDefault();
        if (state === "idle") {
          keyDownTimestampRef.current = Date.now();
          startRecording();
          recordingModeRef.current = "tap";
          holdTimerRef.current = setTimeout(() => {
            recordingModeRef.current = "hold";
          }, 1000);
        } else if (state === "recording" && recordingModeRef.current === "tap") {
          if (holdTimerRef.current) {
            clearTimeout(holdTimerRef.current);
            holdTimerRef.current = null;
          }
          stopRecording();
        }
      } else if (e.code === "Escape") {
        e.preventDefault();
        if (showSettings) {
          setShowSettings(false);
        } else if (state === "results") {
          setState("idle");
        }
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      if (showSettings) return; // Ignore keyboard shortcuts when settings are open

      if (e.code === "Space" && state === "recording") {
        e.preventDefault();
        if (holdTimerRef.current) {
          clearTimeout(holdTimerRef.current);
          holdTimerRef.current = null;
        }
        if (recordingModeRef.current === "hold") {
          stopRecording();
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [state, startRecording, stopRecording, showSettings]);

  useEffect(() => {
    if (state === "recording") {
      timerRef.current = setInterval(() => {
        setRecordingTime((prev) => prev + 1);
      }, 1000);
    } else {
      if (timerRef.current) {
        clearInterval(timerRef.current);
      }
      if (state === "idle") {
        setRecordingTime(0);
      }
    }

    return () => {
      if (timerRef.current) {
        clearInterval(timerRef.current);
      }
    };
  }, [state]);

  useEffect(() => {
    if (!containerRef.current) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (state === "results") {
          const { height } = entry.contentRect;
          WindowSetSize(WINDOW_WIDTH, Math.ceil(height));
        } else {
          WindowSetSize(WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT);
        }
      }
    });
    observer.observe(containerRef.current);
    return () => observer.disconnect();
  }, [state]);

  const formatTime = (seconds: number) => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
  };

  const toggleSettings = () => {
    setShowSettings(!showSettings);
  };

  return (
    <div
      ref={containerRef}
      className="flex flex-col w-full overflow-hidden relative"
      style={{ backgroundColor: colors.base, color: colors.text }}
    >
      {/* Header with minimal controls */}
      <div
        className="flex justify-between items-center px-4 py-2"
        style={{ backgroundColor: colors.mantle }}
      >
        <div className="text-sm font-medium" style={{ color: colors.lavender }}>Dictator</div>
        <button
          onClick={toggleSettings}
          className="p-1 hover:opacity-80 transition-opacity"
          style={{ color: colors.lavender }}
        >
          <Settings size={16} />
        </button>
      </div>

      {/* Settings Panel */}
      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}

      {/* Main content area */}
      <div className="p-4 flex flex-col items-center">
        {/* Recording button */}
        <div className="flex flex-col items-center mb-4">
          <button
            onClick={state === "idle" ? startRecording : state === "recording" ? stopRecording : undefined}
            className="flex items-center justify-center rounded-full w-14 h-14 transition-all duration-300"
            style={{
              backgroundColor: state === "recording" ? colors.red : colors.accent,
              boxShadow: `0 0 10px ${state === "recording" ? colors.red : colors.accent}90`
            }}
          >
            {state === "recording" ? (
              <StopCircle className="h-6 w-6" />
            ) : state === "transcribing" ? (
              <div
                className="animate-spin h-6 w-6 border-2 rounded-full border-b-transparent"
                style={{ borderColor: colors.text, borderBottomColor: 'transparent' }}
              />
            ) : (
              <Mic className="h-6 w-6" style={{ color: colors.base }} />
            )}
          </button>

          <div className="mt-2 text-sm">
            {state === "idle" && (
              <span style={{ color: colors.subtext }}>Press Space to record</span>
            )}
            {state === "transcribing" && (
              <span style={{ color: colors.blue }}>Transcribing...</span>
            )}
          </div>
        </div>

        {/* Waveform visualization (only during recording) */}
        {state === "recording" && (
          <div className="w-full flex flex-col items-center">
            <span
              className="font-mono text-base"
              style={{ color: colors.accent }}
            >
              {formatTime(recordingTime)}
            </span>
          </div>
        )}

        {/* Transcript */}
        {state === "results" && (
          <div className="w-full mt-2">
            <div
              className="p-4 rounded-md text-sm leading-relaxed whitespace-pre-wrap relative group"
              style={{ backgroundColor: colors.surface0 }}
            >
              {transcriptionResult}

              {/* Copy button inside transcript box */}
              <button
                onClick={copyToClipboard}
                className="absolute top-2 right-2 p-1.5 rounded opacity-0 group-hover:opacity-100 transition-opacity"
                style={{
                  backgroundColor: colors.surface1,
                  color: colors.subtext
                }}
              >
                <Copy size={14} />
              </button>
            </div>

            <div
              className="flex justify-center items-center mt-3 text-xs"
              style={{ color: colors.overlay }}
            >
              <div className="flex items-center gap-2">
                <span className="font-mono">{formatTime(finalRecordingTimeRef.current)}</span>
                <span className="mx-2">•</span>
                <span>Press Esc to reset</span>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
