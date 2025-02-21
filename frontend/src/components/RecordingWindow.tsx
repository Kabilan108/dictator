import { useState, useEffect, useRef, useCallback } from "react";
import { Mic } from "lucide-react";

import { cn } from "@/lib/utils";
import { WindowSetSize } from "@wailsjs/runtime";
import { Log } from "@/lib/utils";

type RecordingState = "idle" | "recording" | "transcribing" | "results";

export function RecordingWindow() {
	const [state, setState] = useState<RecordingState>("idle");
	const [recordingTime, setRecordingTime] = useState(0);
	const [transcriptionResult, setTranscriptionResult] = useState<string>("");
	const [audioBlob, setAudioBlob] = useState<Blob | null>(null);
	const mediaRecorderRef = useRef<MediaRecorder | null>(null);
	const mediaStreamRef = useRef<MediaStream | null>(null);
	const timerRef = useRef<NodeJS.Timeout | null>(null);
	const finalRecordingTimeRef = useRef<number>(0);

	// New refs for handling space key press durations
	const keyDownTimestampRef = useRef<number>(0);
	const holdTimerRef = useRef<NodeJS.Timeout | null>(null);
	// "tap" means a quick press (recording won't automatically stop on release)
	// "hold" means press held for >1s (recording stops upon key release)
	const recordingModeRef = useRef<"tap" | "hold" | null>(null);

	// New container ref for monitoring size changes
	const containerRef = useRef<HTMLDivElement>(null);

	const version = navigator.userAgent;
	navigator.mediaDevices.enumerateDevices().then((devices) => {
		Log.i(`Devices: ${JSON.stringify(devices)}`);
	});
	Log.i(`Version: ${version}`);

	const getUserMedia = useCallback(
		(handleStream: (stream: MediaStream) => void) => {
			try {
				const options = { audio: true };
				if (navigator.mediaDevices.getUserMedia) {
					navigator.mediaDevices
						.getUserMedia(options)
						.then((stream) => {
							handleStream(stream);
						})
						.catch((err) => {
							Log.e(`Error accessing microphone: ${err}`);
							setState("idle");
							alert(
								"Microphone access is required for recording. Please allow permission.",
							);
						});
				} else {
					Log.e(
						"getUserMedia: navigator.mediaDevices.getUserMedia not supported",
					);
					alert("Audio recording is not supported in this browser");
				}
			} catch (error) {
				Log.e(error as string);
				setState("idle");
			}
		},
		[],
	);

	// Recording setup: get user media and start recording; keep the stream for the visualizer
	useEffect(() => {
		navigator.mediaDevices.enumerateDevices().then((devices) => {
			Log.i(`Devices (in effect): ${JSON.stringify(devices)}`);
		});

		if (state === "recording") {
			setAudioBlob(null);
			const chunks: BlobPart[] = [];
			getUserMedia((stream) => {
				mediaStreamRef.current = stream;
				const recorder = new MediaRecorder(stream);
				mediaRecorderRef.current = recorder;
				recorder.ondataavailable = (e) => {
					chunks.push(e.data);
				};
				recorder.onstop = () => {
					const blob = new Blob(chunks, { type: "audio/wav" });
					setAudioBlob(blob);
					Log.i(
						`Recording complete: ${JSON.stringify({
							duration: finalRecordingTimeRef.current,
							size: blob?.size,
							type: blob?.type,
						})}`,
					);
					if (mediaStreamRef.current) {
						for (const track of mediaStreamRef.current.getTracks()) {
							track.stop();
						}
						mediaStreamRef.current = null;
					}
				};
				recorder.start();
			});
		}
	}, [state, getUserMedia]);

	if (audioBlob) {
		// no-op
	}

	// Keyboard handling for space button:
	// - If in idle: on space keydown, start recording.
	//   * Default mode is "tap": recording stops on a subsequent space keydown.
	//   * A timer switches the mode to "hold" after 1 second; then recording stops on keyup.
	// - If already recording in "tap" mode: the next space keydown stops recording.
	// - If in "hold" mode, recording stops on space keyup.
	useEffect(() => {
		const handleKeyDown = (e: KeyboardEvent) => {
			// Ignore repeats to avoid multiple triggers for hold
			if (e.code === "Space" && !e.repeat) {
				e.preventDefault();
				if (state === "idle") {
					// Start recording in tap mode
					keyDownTimestampRef.current = Date.now();
					setState("recording");
					recordingModeRef.current = "tap"; // default mode until held >1s
					holdTimerRef.current = setTimeout(() => {
						recordingModeRef.current = "hold";
					}, 1000);
				} else if (
					state === "recording" &&
					recordingModeRef.current === "tap"
				) {
					// In tap mode, a subsequent space keydown stops recording.
					if (holdTimerRef.current) {
						clearTimeout(holdTimerRef.current);
						holdTimerRef.current = null;
					}
					finalRecordingTimeRef.current = recordingTime;
					if (
						mediaRecorderRef.current &&
						mediaRecorderRef.current.state === "recording"
					) {
						mediaRecorderRef.current.stop();
					}
					setState("transcribing");
					// Simulate transcription with mock data
					setTimeout(() => {
						setTranscriptionResult(
							"This is a mock transcription result that demonstrates how the transcribed text will appear in the interface. It shows how the text flows and wraps within the window. The transcription includes multiple sentences to show proper formatting and spacing. This example also helps visualize how longer transcriptions will affect the window size and scrolling behavior. Feel free to adjust the window dimensions to see how the text adapts to different sizes.",
						);
						setState("results");
					}, 2000);
				}
			} else if (e.code === "Escape" && state === "results") {
				e.preventDefault();
				setState("idle");
			}
		};

		const handleKeyUp = (e: KeyboardEvent) => {
			if (e.code === "Space" && state === "recording") {
				e.preventDefault();
				// Clear the hold timer if it exists to prevent it from switching to 'hold' mode after key release
				if (holdTimerRef.current) {
					clearTimeout(holdTimerRef.current);
					holdTimerRef.current = null;
				}
				// If we are in hold mode, stop recording upon key release.
				if (recordingModeRef.current === "hold") {
					finalRecordingTimeRef.current = recordingTime;
					if (
						mediaRecorderRef.current &&
						mediaRecorderRef.current.state === "recording"
					) {
						mediaRecorderRef.current.stop();
					}
					setState("transcribing");
					// Simulate transcription with mock data
					setTimeout(() => {
						setTranscriptionResult(
							"This is a mock transcription result that demonstrates how the transcribed text will appear in the interface. It shows how the text flows and wraps within the window. The transcription includes multiple sentences to show proper formatting and spacing. This example also helps visualize how longer transcriptions will affect the window size and scrolling behavior. Feel free to adjust the window dimensions to see how the text adapts to different sizes.",
						);
						setState("results");
					}, 2000);
				}
			}
		};

		window.addEventListener("keydown", handleKeyDown);
		window.addEventListener("keyup", handleKeyUp);

		return () => {
			window.removeEventListener("keydown", handleKeyDown);
			window.removeEventListener("keyup", handleKeyUp);
		};
	}, [state, recordingTime]);

	// Timer update for recording state
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

	// Update the ResizeObserver effect
	useEffect(() => {
		if (!containerRef.current) return;
		const observer = new ResizeObserver((entries) => {
			for (const entry of entries) {
				// Only adjust height when in results state
				if (state === "results") {
					const { height } = entry.contentRect;
					WindowSetSize(500, Math.ceil(height));
				} else {
					// Reset to minimum height for other states
					WindowSetSize(500, 100);
				}
			}
		});
		observer.observe(containerRef.current);
		return () => {
			observer.disconnect();
		};
	}, [state]);

	// Request permission on mount
	useEffect(() => {
		navigator.mediaDevices.getUserMedia({ audio: true }).catch((err) => {
			Log.w(`Initial permission check failed: ${err}`);
		});
	}, []);

	const formatTime = (seconds: number) => {
		const mins = Math.floor(seconds / 60);
		const secs = seconds % 60;
		return `${mins.toString().padStart(2, "0")}:${secs
			.toString()
			.padStart(2, "0")}`;
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
					state === "results" ? "min-h-[8rem] h-auto" : "h-[140px]",
				)}
			>
				<div className="flex items-center w-full">
					<Mic
						className={cn(
							"w-8 h-8 shrink-0",
							state === "recording" ? "text-green-500" : "text-blue-500",
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
