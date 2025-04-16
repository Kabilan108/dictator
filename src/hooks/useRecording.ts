import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

import { Log } from "@/lib/utils";
import type { RecordingState, Result, TranscriptionResult } from "@/types";

export default function useRecording() {
  const [state, setState] = useState<RecordingState>("idle");
  const [recordingTime, setRecordingTime] = useState(0);
  const [transcript, setTranscript] = useState<string>("");
  const [finalRecordingTime, setFinalRecordingTime] = useState(0);
  const timerRef = useRef<NodeJS.Timeout | null>(null);

  const startRecording = useCallback(async () => {
    try {
      Log.d("Invoking start_recording");
      const result: Result = await invoke("start_recording");
      if (!result.success) {
        throw new Error(result.error || "Unknown error starting recording");
      }
      Log.d("start_recording successful");
      setState("recording");
    } catch (error) {
      Log.e(`Error starting recording: ${error}`);
      setState("idle");
      alert(
        `Failed to start recording: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }, []);

  const stopRecording = useCallback(async () => {
    try {
      setState("transcribing");
      Log.d("Invoking stop_recording");
      const result: TranscriptionResult = await invoke("stop_recording");
      if (!result.success) {
        throw new Error(result.error || "Unknown error stopping recording");
      }
      setTranscript(result.transcript || "");
      setFinalRecordingTime(recordingTime)
      Log.d("stop_recording successful, transcript received.");
      setState("results");
    } catch (error) {
      Log.e(`Error stopping recording: ${error}`);
      setState("idle");
      alert(
        `Failed to stop recording or transcribe: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }, [recordingTime]);

  // timer effect
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

  return {
    state,
    setState,
    recordingTime,
    transcript,
    finalRecordingTime,
    startRecording,
    stopRecording,
  }
}
