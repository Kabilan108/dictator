// main.js

const { app, BrowserWindow, ipcMain } = require('electron');
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { v4: uuidv4 } = require('uuid');

function createTempAudioFile(audioBuffer, callback) {
    const tempDir = os.tmpdir();
    const filePath = path.join(tempDir, `${uuidv4()}.mp3`);

    fs.writeFile(filePath, audioBuffer, (err) => {
        if (err) {
            console.error('Failed to write temp audio to file', err);
            callback(err);
            return;
        }
        callback(null, filePath);
    })
}


const worker = spawn('./worker.py', [], { shell: true });
let isWorkerReady = false;

let win;

function createWindow() {
    win = new BrowserWindow({
        width: 400,
        height: 200,
        webPreferences: {
            preload: path.join(__dirname, 'preload.js')
        }
    });

    win.loadFile('index.html');
    // win.webContents.openDevTools();
}


worker.stdout.on('data', (data) => {
    const message = data.toString().trim();

    if (message === '[ready]') {
        console.log('Worker is ready');
        isWorkerReady = true;
        win?.webContents.send('worker-ready', isWorkerReady);
    }

    if (message.startsWith('[transcript]')) {
        const transcript = message.substring('[transcript]'.length).trim();
        console.log(`Transcript: ${transcript}`);
        win?.webContents.send('transcription', transcript);
    }
})

worker.stderr.on('data', (data) => {
    console.error(`Worker error: ${data}`);
})

worker.on('close', (code) => {
    console.log(`Worker exited with code ${code}`);
})


function sendToWorker(audioBuffer) {
    if (!isWorkerReady) {
        console.error('Worker is not ready');
        return;
    }

    createTempAudioFile(audioBuffer, (err, filePath) => {
        if (err) {
            console.error('Failed to create temp file for audio buffer');
            return;
        }

        console.log(`Transcribing audio file: ${filePath}`);
        worker.stdin.write(`\\transcribe ${filePath}\n`);
    })
}


app.whenReady().then(() => {
    ipcMain.handle('check-worker-ready', async (event) => {
        return isWorkerReady;
    });

    ipcMain.handle('transcribe', async (event, audioBuffer) => {
        console.log('Received transcribe request');
        sendToWorker(audioBuffer);
    })
    
    createWindow();

    win?.webContents.send('worker-ready', isWorkerReady);
});

app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') {
        app.quit();
    }
});

app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
        createWindow();
    }
});
