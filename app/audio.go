package app

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/gordonklaus/portaudio"

  "dictator/util"
)

// wavHeader defines the structure of a WAV file header.
type wavHeader struct {
	ChunkID       [4]byte // "RIFF"
	ChunkSize     uint32  // 36 + SubChunk2Size
	Format        [4]byte // "WAVE"
	Subchunk1ID   [4]byte // "fmt "
	Subchunk1Size uint32  // 16 for PCM
	AudioFormat   uint16  // 1 for PCM, 3 for IEEE Float
	NumChannels   uint16  // 1 for mono, 2 for stereo
	SampleRate    uint32  // 44100
	ByteRate      uint32  // SampleRate * NumChannels * BitsPerSample/8
	BlockAlign    uint16  // NumChannels * BitsPerSample/8
	BitsPerSample uint16  // 32 for float32
	Subchunk2ID   [4]byte // "data"
	Subchunk2Size uint32  // NumSamples * NumChannels * BitsPerSample/8
}

func createWavHeader(dataSize uint32) wavHeader {
	return wavHeader{
		ChunkID:       [4]byte{'R', 'I', 'F', 'F'},
		ChunkSize:     36 + dataSize,
		Format:        [4]byte{'W', 'A', 'V', 'E'},
		Subchunk1ID:   [4]byte{'f', 'm', 't', ' '},
		Subchunk1Size: 16,
		AudioFormat:   3,                    // IEEE Float
		NumChannels:   1,                    // Mono
		SampleRate:    44100,                // 44.1kHz
		BitsPerSample: 32,                   // 32-bit float
		ByteRate:      44100 * 1 * (32 / 8), // SampleRate * NumChannels * (BitsPerSample / 8)
		BlockAlign:    1 * (32 / 8),         // NumChannels * (BitsPerSample / 8)
		Subchunk2ID:   [4]byte{'d', 'a', 't', 'a'},
		Subchunk2Size: dataSize,
	}
}

type AudioRecorder struct {
	stream         *portaudio.Stream
	buffer         []float32
	isRecording    bool
	isInitialized  bool
	sampleRate     int
	framesPerBlock int
	mu             sync.Mutex
	wg             sync.WaitGroup
}

func NewAudioRecorder() (*AudioRecorder, error) {
	recorder := &AudioRecorder{
		buffer:         make([]float32, 0),
		isRecording:    false,
		isInitialized:  false,
		sampleRate:     44100,
		framesPerBlock: 1024,
	}
	if err := recorder.Initialize(); err != nil {
		return nil, fmt.Errorf("failed to initialize audio recorder: %w", err)
	}
	return recorder, nil
}

func (a *AudioRecorder) Initialize() error {
	if a.isInitialized {
		return nil
	}
	if err := portaudio.Initialize(); err != nil {
		return fmt.Errorf("failed to initialize PortAudio: %w", err)
	}
	a.isInitialized = true
	return nil
}

func (a *AudioRecorder) Terminate() error {
	if !a.isInitialized {
		return nil
	}
	if a.isRecording {
		if _, err := a.StopRecording(); err != nil {
			return fmt.Errorf("failed to stop recording before termination: %w", err)
		}
	}
	if err := portaudio.Terminate(); err != nil {
		return fmt.Errorf("failed to terminate portaudio: %w", err)
	}
	a.isInitialized = false
	return nil
}

type AudioDevice struct {
	name      string
	isDefault bool
}

func (a *AudioRecorder) ListDevices() ([]AudioDevice, error) {
	a.mu.Lock()
	if !a.isInitialized {
		a.mu.Unlock()
		return make([]AudioDevice, 0), fmt.Errorf("audio recorder not initialized")
	}
	a.mu.Unlock()

	did, err := portaudio.DefaultInputDevice()
	if err != nil {
		return make([]AudioDevice, 0), fmt.Errorf("failed to get default input device: %w", err)
	}

	paDevices, err := portaudio.Devices()
	if err != nil {
		return make([]AudioDevice, 0), fmt.Errorf("failed to get devices: %w", err)
	}

	ads := make([]AudioDevice, 0)
	for _, dev := range paDevices {
		ads = append(ads, AudioDevice{dev.Name, dev == did})
	}

	return ads, nil
}

func (a *AudioRecorder) StartRecording() error {
	a.mu.Lock()
	if !a.isInitialized {
		a.mu.Unlock()
		return fmt.Errorf("audio recorder not initialized")
	}
	if a.isRecording {
		a.mu.Unlock()
		return fmt.Errorf("recording already in progress")
	}
	a.isRecording = true
	a.mu.Unlock()

	bufferSize := a.framesPerBlock
	framesPerBuffer := make([]float32, bufferSize)

	stream, err := portaudio.OpenDefaultStream(
		1,                     // inputChannels
		0,                     // outputChannels
		float64(a.sampleRate), // sampleRate
		bufferSize,            // framesPerBuffer
		framesPerBuffer,       // buffer
	)
	if err != nil {
		return fmt.Errorf("failed to open audio stream: %w", err)
	}

	if err := stream.Start(); err != nil {
		stream.Close()
		return fmt.Errorf("failed to start audio stream: %w", err)
	}

	a.mu.Lock()
	a.stream = stream
	a.mu.Unlock()

	a.wg.Add(1)
	go func() {
		defer a.wg.Done()
		for {
			a.mu.Lock()
			active := a.isRecording
			a.mu.Unlock()
			if !active {
				break
			}

			if err := a.stream.Read(); err != nil {
				util.Log.W("Error reading audio stream: %v", err)
				a.mu.Lock()
				active = a.isRecording
				a.mu.Unlock()
				if !active {
					break
				}
				continue
			}
			dataCopy := make([]float32, len(framesPerBuffer))
			copy(dataCopy, framesPerBuffer)
			a.mu.Lock()
			a.buffer = append(a.buffer, dataCopy...)
			a.mu.Unlock()
		}
	}()

	return nil
}

func (a *AudioRecorder) StopRecording() ([]byte, error) {
	a.mu.Lock()
	if a.stream != nil {
		a.isRecording = false
	}
	a.mu.Unlock()

	a.wg.Wait()

	a.mu.Lock()
	if a.stream != nil {
		if err := a.stream.Stop(); err != nil {
			return nil, fmt.Errorf("failed to stop recording: %w", err)
		}
		if err := a.stream.Close(); err != nil {
			return nil, fmt.Errorf("failed to close stream: %w", err)
		}
	}

	bufferCopy := make([]float32, len(a.buffer))
	copy(bufferCopy, a.buffer)
	a.buffer = make([]float32, 0)
	a.mu.Unlock()

	var buf bytes.Buffer
	for _, sample := range bufferCopy {
		err := binary.Write(&buf, binary.LittleEndian, sample)
		if err != nil {
			return nil, err
		}
	}
	return buf.Bytes(), nil
}

func WriteWavFile(filePath string, audioData []byte) error {
	// Validate file path
	dir := filepath.Dir(filePath)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		return fmt.Errorf("directory does not exist: %s", dir)
	}

	// Try to create the file
	file, err := os.Create(filePath)
	if err != nil {
		return fmt.Errorf("failed to create output file: %w", err)
	}
	defer file.Close()

	// Write WAV header
	header := createWavHeader(uint32(len(audioData)))
	if err := binary.Write(file, binary.LittleEndian, header); err != nil {
		return fmt.Errorf("failed to write WAV header: %w", err)
	}

	// Write the audio data
	if _, err := file.Write(audioData); err != nil {
		return fmt.Errorf("failed to write audio data: %w", err)
	}

	return nil
}

func TestAudioRecorder() {
	// Create a new audio recorder
	recorder, err := NewAudioRecorder()
	if err != nil {
		util.Log.E("Failed to create audio recorder: %v", err)
		os.Exit(1)
	}
	defer recorder.Terminate()

	// list available devices
	devices, err := recorder.ListDevices()
	if err != nil {
		util.Log.E("Failed to list input devices: %v", err)
		os.Exit(1)
	}

	util.Log.I("Available input devices:")
	for _, d := range devices {
		util.Log.I("  %s (default=%v)", d.name, d.isDefault)
	}

	// Start recording
	err = recorder.StartRecording()
	if err != nil {
		util.Log.E("Failed to start recording: %v", err)
		os.Exit(1)
	}

	util.Log.I("Recording for 5 seconds...")
	time.Sleep(5 * time.Second)

	// Stop recording and get the audio data
	audioData, err := recorder.StopRecording()
	if err != nil {
		util.Log.E("Failed to stop recording: %v", err)
		os.Exit(1)
	}

	if len(audioData) == 0 {
		util.Log.E("No audio data was recorded")
		os.Exit(1)
	}

	// Write the WAV file
	fp, err := util.NewRecordingFile(DATA_DIR)
	if err != nil {
		util.Log.E("Failed to create recording file: %v", err)
		os.Exit(1)
	}

	if err := WriteWavFile(fp, audioData); err != nil {
		util.Log.E("Failed to write WAV file: %v", err)
		os.Exit(1)
	}

	util.Log.I("Successfully recorded audio to %s", fp)
}
