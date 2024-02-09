// src/App.jsx

import { useState } from "react";
import { MicButton } from "./components/mic-button";
import { RecordingIndicator } from "./components/recording-indicator";
import { StatusMessage } from "./components/status-message";

export default function App() {
  const [isRecording, setIsRecording] = useState(false);
  const [statusMessage, setStatusMessage] = useState("--:--:--");

  const toggleRecording = () => {
    setIsRecording(!isRecording);
  };

  return (
    <div className="App">
      <div className="p-4 max-w-sm mx-auto bg-white flex items-center space-x-2">
        <div className="flex-shrink-0">
          <MicButton isRecording={isRecording} onClick={toggleRecording} />
        </div>
        <div>
          <RecordingIndicator isRecording={isRecording} />
          {isRecording ? <StatusMessage message={statusMessage} /> : null}
        </div>
      </div>
    </div>
  );
}
