import React from "react";
import ReactDOM from "react-dom/client";

import { RecordingWindow } from "./components/RecordingWindow";
import "./index.css";
import { ThemeProvider } from "./lib/ThemeContext";

function App() {
  return (
    <ThemeProvider>
      <RecordingWindow />
    </ThemeProvider>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
