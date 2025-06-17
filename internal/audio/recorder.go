package audio

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/gordonklaus/portaudio"
	"github.com/kabilan108/dictator/internal/utils"
)

type RecorderState int

const (
	StateIdle RecorderState = iota
	StateRecording
	StateStopped
)

type Recorder struct {
	stream        *portaudio.Stream
	buffer        []float32
	isInitialized bool

	config utils.AudioConfig

	mu        sync.RWMutex
	state     RecorderState
	audioData []byte
	startTime time.Time

	doneChan  chan struct{}
	errorChan chan error

	durationTimer *time.Timer

	wg sync.WaitGroup
}

func NewRecorder(c utils.AudioConfig, logLevel string) (*Recorder, error) {
	recorder := &Recorder{
		config:        c,
		state:         StateIdle,
		buffer:        make([]float32, 0),
		audioData:     make([]byte, 0),
		doneChan:      make(chan struct{}),
		errorChan:     make(chan error, 1),
		isInitialized: false,
	}

	if err := portaudio.Initialize(); err != nil {
		return nil, fmt.Errorf("failed to initialize PortAudio: %w", err)
	}
	recorder.isInitialized = true

	slog.Debug("recorder initialized", "sr", c.SampleRate, "channels", c.Channels, "bit_depth", c.BitDepth)
	return recorder, nil
}

func (r *Recorder) Close() error {
	r.mu.Lock()
	defer r.mu.Unlock()

	var lastErr error

	if r.state == StateRecording {
		slog.Warn("recorder still active during close, stopping recording")
		_, err := r.stopRecordingUnsafe()
		if err != nil {
			slog.Error("error stopping recording during close", "err", err)
			lastErr = err
		}
	}

	if r.durationTimer != nil {
		r.durationTimer.Stop()
		r.durationTimer = nil
	}

	if r.isInitialized {
		if r.state == StateRecording {
			if _, err := r.stopRecordingUnsafe(); err != nil {
				slog.Error("failed to stop recording before termination", "err", err)
				lastErr = err
			}
		}
		if err := portaudio.Terminate(); err != nil {
			slog.Error("failed to terminate PortAudio", "err", err)
			lastErr = err
		}
		r.isInitialized = false
	}

	// close channels safely
	select {
	case <-r.doneChan:
	default:
		close(r.doneChan)
	}

	slog.Debug("audio recorder closed")
	return lastErr
}

func (r *Recorder) GetState() RecorderState {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.state
}

func (r *Recorder) IsRecording() bool {
	return r.GetState() == StateRecording
}

func (r *Recorder) Start() error {
	r.mu.Lock()
	defer r.mu.Unlock()

	if !r.isInitialized {
		return fmt.Errorf("audio recorder not initialized")
	}
	if r.state == StateRecording {
		return fmt.Errorf("recorder is already recording")
	}

	slog.Debug("starting audio recording")

	// reset audio data buffers
	r.buffer = make([]float32, 0)
	r.audioData = make([]byte, 0)
	r.startTime = time.Now()

	r.durationTimer = time.AfterFunc(
		time.Duration(r.config.MaxDurationMin)*time.Minute,
		func() {
			r.stopRecordingDueToTimeout()
		},
	)

	// create input buffer for portaudio
	framesPerBuffer := make([]float32, r.config.FramesPerBlock)

	// open audio stream
	stream, err := portaudio.OpenDefaultStream(
		1,                            // inputChannels
		0,                            // outputChannels
		float64(r.config.SampleRate), // sampleRate
		r.config.FramesPerBlock,      // framesPerBuffer
		framesPerBuffer,              // buffer
	)
	if err != nil {
		slog.Error("failed to open audio stream", "err", err)
		if r.durationTimer != nil {
			r.durationTimer.Stop()
			r.durationTimer = nil
		}
		return fmt.Errorf("failed to open audio stream: %w", err)
	}

	if err := stream.Start(); err != nil {
		slog.Error("failed to start audio stream", "err", err)
		stream.Close()
		if r.durationTimer != nil {
			r.durationTimer.Stop()
			r.durationTimer = nil
		}
		return fmt.Errorf("failed to start audio stream: %w", err)
	}

	r.stream = stream
	r.state = StateRecording

	r.wg.Add(1)
	go func() {
		defer r.wg.Done()
		for r.IsRecording() {
			if err := r.stream.Read(); err != nil {
				slog.Warn("error reading audio stream", "err", err)
				if !r.IsRecording() {
					break
				}
				continue
			}

			// copy audio data to buffer
			dataCopy := make([]float32, len(framesPerBuffer))
			copy(dataCopy, framesPerBuffer)
			r.mu.Lock()
			r.buffer = append(r.buffer, dataCopy...)
			r.mu.Unlock()
		}
	}()

	slog.Info("recording started", "max_duration_min", r.config.MaxDurationMin)
	return nil
}

func (r *Recorder) Stop() ([]byte, string, error) {
	r.mu.Lock()
	if r.stream != nil {
		r.state = StateStopped
	}
	r.mu.Unlock()

	if r.stream == nil {
		return nil, "", fmt.Errorf("recorder is not recording")
	}

	r.wg.Wait()

	r.mu.Lock()
	defer r.mu.Unlock()

	data, err := r.stopRecordingUnsafe()
	if err != nil {
		slog.Error("error stopping recording", "err", err)
		return nil, "", err
	}

	wavData, err := r.EncodeToWAV(data)
	if err != nil {
		return nil, "", fmt.Errorf("failed to encode to WAV: %w", err)
	}

	rp, err := utils.GetPathToRecording(r.startTime)
	if err != nil {
		return nil, "", err
	}

	slog.Info("recording stopped", "bytes_captured", len(data))
	return wavData, rp, nil
}

// stopRecordingUnsafe stops recording without acquiring the mutex (internal use)
func (r *Recorder) stopRecordingUnsafe() ([]byte, error) {
	if r.durationTimer != nil {
		r.durationTimer.Stop()
		r.durationTimer = nil
	}

	// stop and close the audio stream
	if r.stream != nil {
		if err := r.stream.Stop(); err != nil {
			return nil, fmt.Errorf("failed to stop audio stream: %w", err)
		}
		if err := r.stream.Close(); err != nil {
			return nil, fmt.Errorf("failed to close audio stream: %w", err)
		}
		r.stream = nil
	}

	r.state = StateStopped

	// convert float32 buffer to int16 pcm data
	var buf bytes.Buffer
	for _, sample := range r.buffer {
		// convert float32 (-1.0 to 1.0) to int16 (-32768 to 32767)
		intSample := int16(sample * 32767)
		err := binary.Write(&buf, binary.LittleEndian, intSample)
		if err != nil {
			return nil, fmt.Errorf("failed to convert audio data: %w", err)
		}
	}

	// store converted data in audiodata
	r.audioData = buf.Bytes()

	// Return a copy of the recorded data
	dataCopy := make([]byte, len(r.audioData))
	copy(dataCopy, r.audioData)

	// Clear buffers
	r.buffer = make([]float32, 0)

	return dataCopy, nil
}

func (r *Recorder) GetRecordingDuration() time.Duration {
	r.mu.RLock()
	defer r.mu.RUnlock()

	if r.state != StateRecording {
		return 0
	}

	return time.Since(r.startTime)
}

type WAVHeader struct {
	ChunkID       [4]byte // "RIFF"
	ChunkSize     uint32  // File size - 8
	Format        [4]byte // "WAVE"
	Subchunk1ID   [4]byte // "fmt "
	Subchunk1Size uint32  // 16 for PCM
	AudioFormat   uint16  // 1 for PCM
	NumChannels   uint16  // Number of channels
	SampleRate    uint32  // Sample rate
	ByteRate      uint32  // Sample rate * num channels * bits per sample / 8
	BlockAlign    uint16  // Num channels * bits per sample / 8
	BitsPerSample uint16  // Bits per sample
	Subchunk2ID   [4]byte // "data"
	Subchunk2Size uint32  // Number of bytes in data
}

func (r *Recorder) EncodeToWAV(rawData []byte) ([]byte, error) {
	if len(rawData) == 0 {
		return nil, fmt.Errorf("no audio data to encode")
	}

	// Calculate WAV header values
	numChannels := uint16(r.config.Channels)
	sampleRate := uint32(r.config.SampleRate)
	bitsPerSample := uint16(r.config.BitDepth)
	byteRate := sampleRate * uint32(numChannels) * uint32(bitsPerSample) / 8
	blockAlign := numChannels * bitsPerSample / 8
	dataSize := uint32(len(rawData))

	// Create WAV header
	header := WAVHeader{
		ChunkID:       [4]byte{'R', 'I', 'F', 'F'},
		ChunkSize:     36 + dataSize,
		Format:        [4]byte{'W', 'A', 'V', 'E'},
		Subchunk1ID:   [4]byte{'f', 'm', 't', ' '},
		Subchunk1Size: 16,
		AudioFormat:   1, // PCM
		NumChannels:   numChannels,
		SampleRate:    sampleRate,
		ByteRate:      byteRate,
		BlockAlign:    blockAlign,
		BitsPerSample: bitsPerSample,
		Subchunk2ID:   [4]byte{'d', 'a', 't', 'a'},
		Subchunk2Size: dataSize,
	}

	// Create buffer for WAV file
	var buf bytes.Buffer

	// Write header
	err := binary.Write(&buf, binary.LittleEndian, header)
	if err != nil {
		return nil, fmt.Errorf("failed to write WAV header: %w", err)
	}

	// Write audio data
	_, err = buf.Write(rawData)
	if err != nil {
		return nil, fmt.Errorf("failed to write audio data: %w", err)
	}

	return buf.Bytes(), nil
}

func WriteAudioData(filePath string, audioData []byte) (*os.File, error) {
	dir := filepath.Dir(filePath)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		return nil, fmt.Errorf("directory does not exist: %s", dir)
	}

	file, err := os.Create(filePath)
	if err != nil {
		return nil, fmt.Errorf("failed to create output file: %w", err)
	}
	defer file.Close()

	if _, err := file.Write(audioData); err != nil {
		return nil, fmt.Errorf("failed to write audio data: %w", err)
	}

	return file, nil
}

func (r *Recorder) stopRecordingDueToTimeout() {
	r.mu.Lock()
	if r.stream != nil {
		r.state = StateStopped
	}
	r.mu.Unlock()

	if r.stream == nil {
		return
	}

	slog.Warn("recording stopped due to timeout", "max_duration_min", r.config.MaxDurationMin)

	r.wg.Wait()

	r.mu.Lock()
	defer r.mu.Unlock()

	data, err := r.stopRecordingUnsafe()
	if err != nil {
		slog.Error("error during timeout stop", "err", err)
	} else {
		slog.Info("timeout stop completed", "bytes_captured", len(data))
	}

	timeoutErr := fmt.Errorf("recording stopped: maximum duration of %v min exceeded", r.config.MaxDurationMin)
	select {
	case r.errorChan <- timeoutErr:
	default:
		slog.Warn("error channel full, timeout error not sent")
	}
}

// HasTimedOut returns true if the recording has exceeded the maximum duration
func (r *Recorder) HasTimedOut() bool {
	r.mu.RLock()
	defer r.mu.RUnlock()

	if r.state != StateRecording {
		return false
	}

	return time.Since(r.startTime) >= time.Duration(r.config.MaxDurationMin)*time.Minute
}
