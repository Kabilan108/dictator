import React from "react";
import ReactDOM from "react-dom/client";

import { RecordingWindow } from "./components/RecordingWindow";
import "./index.css";

function App() {
	return <RecordingWindow />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		<App />
	</React.StrictMode>,
);
