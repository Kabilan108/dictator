// preload.js

// import { contextBridge, ipcRenderer } from "electron"
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("nodeAPI", {
  sendTranscribeRequest: (audioBuffer) => {
    ipcRenderer.invoke("transcribe", audioBuffer);
  },
  checkWorker: () => {
    ipcRenderer.invoke("check-worker");
  },
  onTranscription: (callback) => {
    ipcRenderer.on("transcription", callback);
  },
  onWorkerReady: (callback) => {
    ipcRenderer.on("worker-ready", callback);
  },
  Buffer: Buffer,
});
