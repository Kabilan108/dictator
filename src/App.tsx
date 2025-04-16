import { useState, useEffect, useRef, useCallback, RefObject } from "react";
import { Mic, Copy, Settings, Square, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize  } from "@tauri-apps/api/window"

import SettingsPanel from "@/components/SettingsPanel";
import { useTheme } from "@/lib/ThemeContext";
import { Log, formatTime } from "@/lib/utils";
import { SimpleResult, TranscriptionResult } from "@/types";

type RecordingState = "idle" | "recording" | "transcribing" | "results";

// Define constants (consider moving to a config file or utils)
const WINDOW_WIDTH = 375; // Match your tauri.conf.json
const DEFAULT_WINDOW_HEIGHT = 180;
const MAX_WINDOW_HEIGHT = 350;
const MIN_RESULTS_HEIGHT = 250;
const SETTINGS_WINDOW_HEIGHT = 400; // Or adjust as needed

// handle to get current Tauri window
const appWindow = getCurrentWindow();

const StatusText = ({ text, color }: { text: string, color: string }) => {
  return (
    <div className="w-full flex flex-col items-center mt-2 text-sm">
      <span style={{ color: color }}>{text}</span>
    </div>
  )
}

const RecordButton = ({ state, onStartRecording, onStopRecording }: {
  state: RecordingState,
  onStartRecording: () => Promise<void>,
  onStopRecording: () => Promise<void>,
}) => {
  const { colors } = useTheme()

  return (
    <div className="flex flex-col items-center mt-2 mb-4">
      <button
        onClick={state === "idle" ? onStartRecording : state === "recording" ? onStopRecording : undefined}
        className="flex items-center justify-center rounded-full w-14 h-14 transition-all duration-300"
        style={{
          backgroundColor: state === "recording" ? colors.pink : colors.accent,
          boxShadow: `0 0 10px ${state === "recording" ? colors.pink : colors.accent}90`
        }}
      >
        {state === "recording" ? (
          <Square className="h-6 w-6" style={{ color: colors.surface1, fill: colors.surface1 }} />
        ) : state === "transcribing" ? (
          <div
            className="animate-spin h-6 w-6 border-2 rounded-full border-b-transparent"
            style={{ borderColor: colors.text, borderBottomColor: 'transparent' }}
          />
        ) : (
          <Mic className="h-6 w-6" style={{ color: colors.base }} />
        )}
      </button>
    </div>
  )
}

const Header = ({ showSettings, onToggleSettings }: {
  showSettings: boolean,
  onToggleSettings: () => void
}) => {
  const { colors } = useTheme()

  return (
    <div
      className="flex justify-between items-center px-4 py-2"
      style={{ backgroundColor: colors.mantle }}
    >
      <div className="flex items-center text-sm font-medium" style={{ color: colors.lavender }}>
        {showSettings && <Settings size={16} className="mr-2" />} Dictator
      </div>
      <button
        onClick={onToggleSettings}
        className="p-1 hover:opacity-80 transition-opacity"
        style={{ color: colors.lavender }}
      >
        {showSettings ? <X size={16} /> : <Settings size={16} />}
      </button>
    </div>
  )
}

// Transcript text area with dynamic height and scrolling
const TranscriptContainer = ({ transcript, transcriptRef, onCopy }: {
  transcript: string,
  transcriptRef: RefObject<HTMLDivElement>,
  onCopy: () => void,
}) => {
  const { colors } = useTheme()

  return (
    <div
      className="rounded-md text-sm relative group flex-grow"
      style={{ backgroundColor: colors.surface0 }}
    >
      <div
        ref={transcriptRef}
        className="p-4 leading-relaxed whitespace-pre-wrap overflow-y-auto"
        style={{
          maxHeight: "calc(100vh - 200px)",  // Reduced max height to ensure footer visibility
          minHeight: "80px",  // Minimum height to always show some content
          overflowY: "auto"   // Ensure scrollbar appears when needed
        }}
      >
        {transcript}
      </div>

      {/* Copy button */}
      <button
        onClick={onCopy}
        className="absolute top-2 right-2 p-1.5 rounded opacity-0 group-hover:opacity-100 transition-opacity"
        style={{
          backgroundColor: colors.surface1,
          color: colors.subtext
        }}
      >
        <Copy size={14} />
      </button>
    </div>
  )
}

const ResultsFooter = ({ duration }: { duration: number }) => {
  const { colors } = useTheme()

  return (
    <div
      className="flex justify-center items-center mt-2 pt-2 pb-4 text-xs flex-shrink-0"
      style={{
        color: colors.overlay,
        paddingBottom: "8px" // Extra bottom padding to ensure visibility
      }}
    >
      <div className="flex items-center gap-2">
        <span className="font-medium">{formatTime(duration)}</span>
        <span className="mx-2">•</span>
        <span>Press Esc to reset</span>
      </div>
    </div>
  )
}

const App = () => {
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
  const transcriptRef = useRef<HTMLDivElement>(null);

  const startRecording = useCallback(async () => {
    try {
      Log.d("Invoking start_recording");
      // Use invoke, passing no arguments
      const result: SimpleResult = await invoke("start_recording");
      if (!result.success) {
        throw new Error(result.error || "Unknown error starting recording");
      }
      Log.d("start_recording successful");
      setState("recording");
    } catch (error) {
      Log.e(`Error starting recording: ${error}`);
      setState("idle");
      // Provide more context in the alert
      alert(`Failed to start recording: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  const stopRecording = useCallback(async () => {
    try {
      setState("transcribing");
      Log.d("Invoking stop_recording");
      // Use invoke, passing no arguments
      const result: TranscriptionResult = await invoke("stop_recording");
      if (!result.success) {
        throw new Error(result.error || "Unknown error stopping recording");
      }
      finalRecordingTimeRef.current = recordingTime;
      setTranscriptionResult(result.transcript || "");
      Log.d("stop_recording successful, transcript received.");
      setState("results");
    } catch (error) {
      Log.e(`Error stopping recording: ${error}`);
      setState("idle");
      alert(`Failed to stop recording or transcribe: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [recordingTime]);

  const copyToClipboard = useCallback(() => {
    navigator.clipboard.writeText(transcriptionResult)
      .then(() => Log.i("Transcript copied to clipboard"))
      .catch(err => Log.e(`Failed to copy: ${err}`));
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

  // Dynamic window sizing logic based on transcript content
  useEffect(() => {
    const resizeWindow = async (width: number, height: number) => {
      try {
        Log.d(`Resizing window to ${width}x${height}`);
        await appWindow.setSize(new LogicalSize(width, height));
      } catch (e) {
        Log.e("Failed to resize window:", e);
      }
    };

    if (showSettings) {
      resizeWindow(WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT);
    } else if (state === "idle" || state === "recording" || state === "transcribing") {
      resizeWindow(WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT);
    } else if (state === "results") {
      // Keep your existing logic, just replace WindowSetSize with resizeWindow
      resizeWindow(WINDOW_WIDTH, MIN_RESULTS_HEIGHT);
      setTimeout(() => {
        if (transcriptRef.current) {
          const contentHeight = transcriptRef.current.scrollHeight;
          const footerHeight = 50;
          const headerHeight = 40;
          const paddingSpace = 30;
          const necessaryHeight = Math.min(
            contentHeight + headerHeight + footerHeight + paddingSpace,
            MAX_WINDOW_HEIGHT
          );
          const newHeight = Math.max(MIN_RESULTS_HEIGHT, necessaryHeight);
          // Add a small buffer just in case calculation is slightly off
          resizeWindow(WINDOW_WIDTH, newHeight + 10);
        }
      }, 100); // Delay might need adjustment
    }
  }, [state, transcriptionResult, showSettings]);

  const toggleSettings = () => {
    setShowSettings(!showSettings);
  };

  return (
    <div
      ref={containerRef}
      className="flex flex-col w-full h-full overflow-hidden relative flex-grow"
      style={{ backgroundColor: colors.base, color: colors.text }}
    >
      {/* Header with minimal controls */}
      <Header showSettings={showSettings} onToggleSettings={toggleSettings} />


      {/* Main content area with padding adjustments to ensure footer visibility */}
      <div className="px-4 pt-2 pb-2 flex flex-col flex-1">
        {showSettings ? <SettingsPanel /> : (
          <>
            {/* Recording button */}
            <RecordButton state={state} onStartRecording={startRecording} onStopRecording={stopRecording} />

            {state === "idle" && <StatusText text={"Press Space to record"} color={colors.subtext} />}
            {state === "transcribing" && <StatusText text={"Transcribing..."} color={colors.accent} />}
            {state === "recording" && <StatusText text={formatTime(recordingTime)} color={colors.accent} />}

            {/* Results area with dynamic sizing and reliable scrolling */}
            {state === "results" && (
              <div className="w-full mt-2 flex flex-col flex-grow flex-shrink-0">
                {/* Transcript text area with dynamic height and scrolling */}
                <TranscriptContainer
                  transcript={transcriptionResult}
                  transcriptRef={transcriptRef}
                  onCopy={copyToClipboard}
                />

                {/* Time and reset instructions - always at bottom with extra padding */}
                <ResultsFooter duration={finalRecordingTimeRef.current} />
              </div>
            )}
          </>
        )}

      </div>
    </div>
  );
}
export default App;
