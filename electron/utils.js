// utils.js

import { spawn } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import fs from "fs";
import os from "os";

export const getFilename = () => {
  return fileURLToPath(import.meta.url);
};

export const getDirname = () => {
  return path.dirname(getFilename());
};

export function createTempAudioFile(audioBuffer, callback) {
  const tempDir = path.join(os.tmpdir(), "dictator");
  if (!fs.existsSync(tempDir)) {
    fs.mkdirSync(tempDir);
  }
  const date = new Date();
  const fileName = `${date.getDate().toString().padStart(2, "0")}${(
    date.getMonth() + 1
  )
    .toString()
    .padStart(2, "0")}${date.getFullYear()}-${date
    .getHours()
    .toString()
    .padStart(2, "0")}${date.getMinutes().toString().padStart(2, "0")}${date
    .getSeconds()
    .toString()
    .padStart(2, "0")}.mp3`;
  const filePath = path.join(tempDir, fileName);

  fs.writeFile(filePath, audioBuffer, (err) => {
    if (err) {
      console.error("Failed to write temp audio to file", err);
      callback(err);
      return;
    }
    callback(null, filePath);
  });
}

export function deleteAllRecordings() {
  const tempDir = path.join(os.tmpdir(), "dictator");
  if (fs.existsSync(tempDir)) {
    fs.readdir(tempDir, (err, files) => {
      if (err) throw err;

      for (const file of files) {
        fs.unlink(path.join(tempDir, file), (err) => {
          if (err) throw err;
        });
      }
    });
  }
}

export function spawnWorker() {
  const workerPath = path.join(getDirname(), "whisper", "worker.py");
  const worker = spawn(workerPath, [], { shell: true });
  return worker;
}

export const status = {
  STOPPED: "stopped",
  RUNNING: "running",
  READY: "ready",
  SETUP: "setup",
};
