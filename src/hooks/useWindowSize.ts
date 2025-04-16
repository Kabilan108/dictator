import { useEffect } from "react";
import type { RefObject } from "react"
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

import { WINDOW } from "@/config";
import { Log } from "@/lib/utils";
import type { RecordingState } from "@/types";

export default function useWindowSize(
  state: RecordingState,
  transcript: string,
  showSettings: boolean,
  transcriptRef: RefObject<HTMLDivElement>
): void {
  const appWindow = getCurrentWindow();

  useEffect(() => {
    const resizeWindow = async (width: number, height: number) => {
      try {
        await appWindow.setSize(new LogicalSize(width, height));
      } catch (e) {
        Log.e("Failed to resize window:", e);
      }
    };

    if (showSettings) {
      resizeWindow(WINDOW.WIDTH, WINDOW.SETTINGS_HEIGHT)
    } else if (state == "idle" || state == "recording" || state == "transcribing") {
      resizeWindow(WINDOW.WIDTH, WINDOW.HEIGHT)
    } else if (state == "results") {
      // collapse to minimum, then re-measure
      resizeWindow(WINDOW.WIDTH, WINDOW.MIN_RESULTS_HEIGHT)
      setTimeout(() => {
        if (transcriptRef.current) {
          const contentHeight = transcriptRef.current.scrollHeight;
          const headerHeight = 40;
          const footerHeight = 50;
          const paddingSpace = 30;
          const necessaryHeight = Math.min(
            contentHeight + headerHeight + footerHeight + paddingSpace,
            WINDOW.MAX_HEIGHT
          );
          const newHeight = Math.max(WINDOW.MIN_RESULTS_HEIGHT, necessaryHeight);
          resizeWindow(WINDOW.WIDTH, newHeight + 10); // add padding to handle rounding
        }
      }, 100);
    }
  }, [state, transcript, showSettings, transcriptRef]);
}
