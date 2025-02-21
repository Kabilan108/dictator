import { useState, useEffect, useRef, useCallback } from "react";
import { Mic } from "lucide-react";
import { cn } from "@/lib/utils";
import { WindowSetSize } from "@wailsjs/runtime";
import { Log } from "@/lib/utils";
import { StartRecording, StopRecording } from "@wailsjs/go/main/App"; // Import Wails bindings

type RecordingState = "idle" | "recording" | "transcribing" | "results";

export function RecordingWindow() {
  const [state, setState] = useState<RecordingState>("idle");
  const [recordingTime, setRecordingTime] = useState(0);
  const [transcriptionResult, setTranscriptionResult] = useState<string>("");
  const timerRef = useRef<NodeJS.Timeout | null>(null);
  const finalRecordingTimeRef = useRef<number>(0);
  const keyDownTimestampRef = useRef<number>(0);
  const holdTimerRef = useRef<NodeJS.Timeout | null>(null);
  const recordingModeRef = useRef<"tap" | "hold" | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const version = navigator.userAgent;
  Log.i(`Version: ${version}`);

  // Start recording using Wails backend
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

  // Stop recording using Wails backend
  const stopRecording = useCallback(async () => {
    try {
      const result = await StopRecording();
      if (!result.success) {
        throw new Error(result.error || "Unknown error stopping recording");
      }
      finalRecordingTimeRef.current = recordingTime;
      setState("transcribing");
      // Simulate transcription (replace with actual transcription logic later)
      setTimeout(() => {
        setTranscriptionResult(
          "This is a mock transcription result that demonstrates how the transcribed text will appear in the interface. It shows how the text flows and wraps within the window. The transcription includes multiple sentences to show proper formatting and spacing. This example also helps visualize how longer transcriptions will affect the window size and scrolling behavior. Feel free to adjust the window dimensions to see how the text adapts to different sizes."
        );
        setState("results");
      }, 2000);
    } catch (error) {
      Log.e(`Error stopping recording: ${error}`);
      setState("idle");
      alert("Failed to stop recording. Please try again.");
    }
  }, [recordingTime]);

  // Keyboard handling
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
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
      } else if (e.code === "Escape" && state === "results") {
        e.preventDefault();
        setState("idle");
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
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
  }, [state, startRecording, stopRecording]);

  // Timer for recording duration
  useEffect(() => {
    if (state === "recording") {
      timerRef.current = setInterval(() => {
        setRecordingTime((prev) => prev + 1);
      }, 1000);
    } else {
      if (timerRef.current) {
        clearInterval(timerRef.current);
      }
      setRecordingTime(0);
    }

    return () => {
      if (timerRef.current) {
        clearInterval(timerRef.current);
      }
    };
  }, [state]);

  // Window resizing
  useEffect(() => {
    if (!containerRef.current) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (state === "results") {
          const { height } = entry.contentRect;
          WindowSetSize(500, Math.ceil(height));
        } else {
          WindowSetSize(500, 100);
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

  return (
    <div
      ref={containerRef}
      className="w-screen items-center bg-background text-foreground dark"
    >
      <div
        className={cn(
          "bg-background transition-all duration-300 ease-in-out p-6",
          "w-full flex items-center",
          state === "results" ? "min-h-[8rem] h-auto" : "h-[140px]"
        )}
      >
        <div className="flex items-center w-full">
          <Mic
            className={cn(
              "w-8 h-8 shrink-0",
              state === "recording" ? "text-green-500" : "text-blue-500"
            )}
          />
          <div className="flex-1 flex flex-col ml-4">
            {state === "results" ? (
              <>
                <p className="text-sm text-foreground mb-4 text-left">
                  {transcriptionResult}
                </p>
                <span className="text-muted-foreground font-mono mb-2 text-center">
                  {formatTime(finalRecordingTimeRef.current)}
                </span>
                <p className="text-xs text-muted-foreground text-center">
                  Press &lt;esc&gt; to reset
                </p>
              </>
            ) : (
              <>
                {state === "idle" && (
                  <p className="text-muted-foreground text-center">
                    Press and hold &lt;space&gt; to record
                  </p>
                )}
                {state === "recording" && (
                  <>
                    <span className="text-green-500 font-mono text-center">
                      {formatTime(recordingTime)}
                    </span>
                    {recordingModeRef.current === "tap" && (
                      <p className="text-muted-foreground text-sm mt-1 text-center">
                        Press &lt;space&gt; again to stop recording
                      </p>
                    )}
                    {recordingModeRef.current === "hold" && (
                      <p className="text-muted-foreground text-sm mt-1 text-center">
                        Release &lt;space&gt; to stop recording
                      </p>
                    )}
                  </>
                )}
                {state === "transcribing" && (
                  <div className="flex items-center justify-center space-x-2">
                    <div className="animate-spin rounded-full h-4 w-4 border-2 border-primary border-t-transparent" />
                    <span className="text-primary">Transcribing</span>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
