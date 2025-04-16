import { RefObject } from "react";
import { Copy } from "lucide-react";

import { useTheme } from "@/contexts/ThemeContext";
import { formatTime } from "@/lib/utils";

export interface TranscriptProps {
  transcript: string;
  duration: number;
  onCopy: () => void;
  transcriptRef: RefObject<HTMLDivElement>;
}

export default function Transcript({
  transcript,
  duration,
  onCopy,
  transcriptRef
}: TranscriptProps) {
  const { colors } = useTheme();
  return (
    <div className="w-full mt-2 flex flex-col flex-grow flex-shrink-0">
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
      <div
        className="flex justify-center items-center mt-2 pt-2 pb-4 text-xs flex-shrink-0"
        style={{ color: colors.overlay, paddingBottom: "8px" }}
      >
        <div className="flex items-center gap-2">
          <span className="font-medium">{formatTime(duration)}</span>
          <span className="mx-2">•</span>
          <span>Press Esc to reset</span>
        </div>
      </div>
    </div>
  )
}
