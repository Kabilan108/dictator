// preload.js

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('versions', {
    node: () => process.versions.node,
    chrome: () => process.versions.chrome,
    electron: () => process.versions.electron,
})

contextBridge.exposeInMainWorld('electronAPI', {
    sendTranscribeRequest: (audioBuffer) => ipcRenderer.invoke('transcribe', audioBuffer),
    onTranscription: (callback) => ipcRenderer.on('transcription', callback),
    checkWorkerReady: () => ipcRenderer.invoke('check-worker-ready'),
    onWorkerReady: (callback) => ipcRenderer.on('worker-ready', callback),
    convertArrayBufferToBuffer: (arrayBuffer) => {
        return Buffer.from(arrayBuffer)
    }
})