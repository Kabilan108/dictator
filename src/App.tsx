import { useState, useRef, useCallback } from "react";
import { Settings, X } from "lucide-react";

import Recorder from "@/components/Recorder";
import SettingsPanel from "@/components/SettingsPanel";
import Transcript from "@/components/Transcript";
import useKeyboardControls from "@/hooks/useKeyboardControls";
import useWindowSize from "@/hooks/useWindowSize";
import useRecording from "@/hooks/useRecording";
import { useTheme } from "@/contexts/ThemeContext";
import { Log } from "@/lib/utils";

export default function App() {
  const { colors } = useTheme();
  const {
    state,
    setState,
    recordingTime,
    transcript,
    finalRecordingTime,
    startRecording,
    stopRecording,
  } = useRecording();
  const [showSettings, setShowSettings] = useState(false);
  const transcriptRef = useRef<HTMLDivElement>(null);

  const copyToClipboard = useCallback(() => {
    navigator.clipboard.writeText(transcript)
      .then(() => Log.i("Transcript copied to clipboard"))
      .catch(err => Log.e(`Failed to copy: ${err}`));
  }, [transcript]);

  useWindowSize(state, transcript, showSettings, transcriptRef);

  useKeyboardControls({
    state,
    showSettings,
    onStart: startRecording,
    onStop: stopRecording,
    onReset: () => setState("idle"),
  })

  return (
    <div
      className="flex flex-col w-full h-full overflow-hidden relative flex-grow"
      style={{ backgroundColor: colors.base, color: colors.text }}
    >
      <div
        className="flex justify-between items-center px-4 py-2"
        style={{ backgroundColor: colors.mantle }}
      >
        <div
          className="flex items-center text-sm font-medium"
          style={{ color: colors.lavender }}
        >
          {showSettings && <Settings size={16} className="mr-2" />} Dictator
        </div>
        <button
          onClick={() => setShowSettings(!showSettings)}
          className="p-1 hover:opacity-80 transition-opacity"
          style={{ color: colors.lavender }}
        >
          {showSettings ? <X size={16} /> : <Settings size={16} />}
        </button>
      </div>
      <div className="px-4 pt-2 pb-2 flex flex-col flex-1">
        {showSettings ? <SettingsPanel /> : (
          <>
            <Recorder
              state={state}
              time={recordingTime}
              onStart={startRecording}
              onStop={stopRecording}
            />
            {state == "results" && (
              <Transcript
                transcript={transcript}
                duration={finalRecordingTime}
                onCopy={copyToClipboard}
                transcriptRef={transcriptRef}
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}
