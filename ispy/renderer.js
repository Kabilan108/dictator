// renderer.js

const startBtn = document.getElementById('startBtn');
const stopBtn = document.getElementById('stopBtn');
const statusSpan = document.getElementById('status');
const audioPlayback = document.getElementById('audioPlayback');

let mediaRecorder;
let audioChunks = [];


document.addEventListener('DOMContentLoaded', async () => {
    const isWorkerReady = await electronAPI.checkWorkerReady();
    updateButtonState(isWorkerReady);

    electronAPI.onWorkerReady((event, isReady) => {
        updateButtonState(isReady);
    })
})

function updateButtonState(isReady) {
    startBtn.disabled = !isReady;
    stopBtn.disabled = true;
    statusSpan.textContent = isReady ? 'Idle' : 'Loading...';
}

electronAPI.onTranscription((event, transcript) => {
    let Transcript = document.getElementById('transcript')
    Transcript.hidden = false;
    Transcript.textContent = transcript;
    audioPlayback.hidden = false;
})


startBtn.addEventListener('click', async () => {
    startBtn.disabled = true;
    stopBtn.disabled = false;
    statusSpan.textContent = 'Recording...';
    audioPlayback.hidden = true;

    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    mediaRecorder = new MediaRecorder(stream);

    mediaRecorder.ondataavailable = event => {
        audioChunks.push(event.data);
    };

    mediaRecorder.onstop = () => {
        const audioBlob = new Blob(audioChunks, { type: 'audio/mpeg' });
        audioChunks = [];
        const audioUrl = URL.createObjectURL(audioBlob);
        audioPlayback.src = audioUrl;
        audioPlayback.hidden = false;

        audioBlob.arrayBuffer().then(buffer => {
            const audioBuffer = electronAPI.convertArrayBufferToBuffer(buffer);
            electronAPI.sendTranscribeRequest(audioBuffer);
        })
    };

    mediaRecorder.start();
});

stopBtn.addEventListener('click', () => {
    stopBtn.disabled = true;
    startBtn.disabled = false;
    statusSpan.textContent = 'Idle';
    mediaRecorder.stop();
});

