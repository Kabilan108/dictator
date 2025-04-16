import { Mic, Square } from "lucide-react";

import { formatTime } from "@/lib/utils";
import { useTheme } from "@/contexts/ThemeContext";
import type { RecordingState } from "@/types";

export interface RecorderProps {
  state: RecordingState;
  time: number;
  onStart: () => Promise<void>;
  onStop: () => Promise<void>;
}

const StatusText = ({ text, color }: { text: string, color: string }) => (
  <div className="w-full flex flex-col items-center mt-2 text-sm">
    <span style={{ color }}>{text}</span>
  </div>
)

const RecordButton = ({ state, onStart, onStop }: {
  state: RecordingState,
  onStart: () => Promise<void>,
  onStop: () => Promise<void>,
}) => {
  const { colors } = useTheme()
  return (
    <div className="flex flex-col items-center mt-2 mb-4">
      <button
        onClick={
          state === "idle" ? onStart
            : state === "recording" ? onStop
              : undefined
        }
        className="flex items-center justify-center rounded-full w-14 h-14 transition-all duration-300"
        style={{
          backgroundColor: state === "recording" ? colors.pink : colors.accent,
          boxShadow: `0 0 10px ${state === "recording" ? colors.pink : colors.accent}90`
        }}
      >
        {state === "recording" ? (
          <Square
            className="h-6 w-6"
            style={{ color: colors.surface1, fill: colors.surface1 }}
          />
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

export default function Recorder({
  state,
  time,
  onStart,
  onStop,
}: RecorderProps) {
  const { colors } = useTheme()
  return (
    <>
      <RecordButton state={state} onStart={onStart} onStop={onStop} />
      {state === "idle" && (
        <StatusText text={"Press Space to record"} color={colors.subtext} />
      )}
      {state === "transcribing" && (
        <StatusText text={"Transcribing..."} color={colors.accent} />
      )}
      {state === "recording" && (
        <StatusText text={formatTime(time)} color={colors.accent} />
      )}
    </>
  )
}
