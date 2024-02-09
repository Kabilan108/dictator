// main.js

import { app, BrowserWindow, Tray, ipcMain } from "electron";
import path from "path";
import fs from "fs";
import * as utils from "./utils.js";

let tray;
let window;
let worker;
let workerStatus = utils.status.STOPPED;
let saveRecording = false;

function createWindow() {
  window = new BrowserWindow({
    width: 300,
    height: 150,
    webPreferences: {
      preload: path.join(utils.getDirname(), "preload.cjs"),
      nodeIntegration: true,
      contextIsolation: true,
    },
  });
  tray = new Tray(path.join(utils.getDirname(), "assets", "icon.png"));

  const startURL =
    process.env.ENV === "development"
      ? "http://localhost:3000"
      : `file://${path.join(__dirname, "build/index.html")}`;

  window.loadURL(startURL);

  window.on("closed", () => (window = null));

  // TODO: implement tray behavior
}

function createWorker() {
  worker = utils.spawnWorker();

  worker.stdout.on("data", (data) => {
    const message = data.toString().trim();

    if (message == "[ready]") {
      console.log("Worker is ready!");

      workerStatus = utils.status.READY;
      window?.webContents.send("worker-ready", workerStatus);
    }

    if (message.startsWith("[transcription]")) {
      const transcription = message.replace("[transcription]", "").trim();
      window?.webContents.send("transcription", transcription);
    }
  });

  worker.stderr.on("data", (data) => {
    console.error(`Worker stderr: ${data}`);
  });

  worker.on("close", (code, signal) => {
    console.log(`Worker exited with code ${code} and signal ${signal}`);
  });
}

function sendToWorker(audioBuffer) {
  if (workerStatus !== utils.status.READY) {
    // TODO send message to user
    console.error(`Worker is not ready`);
    return;
  }

  utils.createTempAudioFile(audioBuffer, (err, filePath) => {
    if (err) {
      console.error(`Failed to create temp file`, err);
      return;
    }

    // check if file exists
    if (!fs.existsSync(filePath)) {
      console.error(`File does not exist: ${filePath}`);
      return;
    }

    console.log(`Transcribing ${filePath}`);
    worker.stdin.write(`\\transcribe ${filePath}\n`);
  });
}

app.whenReady().then(() => {
  createWorker();
  createWindow();

  ipcMain.handle("check-worker", async () => {
    return workerStatus;
  });

  ipcMain.handle("transcribe", async (event, audioBuffer) => {
    sendToWorker(audioBuffer);
  });

  window?.webContents.send("worker-ready", workerStatus);
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
    if (!saveRecording) utils.deleteAllRecordings();
  }
});

app.on("activate", () => {
  if (window === null) {
    createWindow();
  }
});
