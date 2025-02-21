package main

// TODO: make this better for longer recordings
//       when the recording > THRESHOLD sec long -> split and start
//       transcribing each part

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/google/uuid"
)

// App struct
type App struct {
	ctx context.Context
	ar  *AudioRecorder
}

// NewApp creates a new App application struct
func NewApp() *App {
	return &App{}
}

// startup is called at application startup
func (a *App) startup(ctx context.Context) {
	ar, err := NewAudioRecorder()
	if err != nil {
		panic(fmt.Sprintf("failed to start audio recorder: %v", err))
	}

	a.ctx = ctx
	a.ar = ar
}

// domReady is called after front-end resources have been loaded
func (a App) domReady(ctx context.Context) {
	// Add your action here
}

// beforeClose is called when the application is about to quit,
// either by clicking the window close button or calling runtime.Quit.
// Returning true will cause the application to continue, false will continue shutdown as normal.
func (a *App) beforeClose(ctx context.Context) (prevent bool) {
	return false
}

// shutdown is called at application termination
func (a *App) shutdown(ctx context.Context) {
	a.ar.Terminate()
}

// start a recording
func (a *App) StartRecording() bool {
	if err := a.ar.StartRecording(); err != nil {
		Log.E("Failed to start recording:", err)
		return false
	}
	Log.D("Started recording")
	return true
}

// // Validate file path
// dir := filepath.Dir(filePath)
// if _, err := os.Stat(dir); os.IsNotExist(err) {
// 	return fmt.Errorf("directory does not exist: %s", dir)
// }

// stop a recording
func (a *App) StopRecording() bool {
	data, err := a.ar.StopRecording()
	fp := filepath.Join(DATA_DIR, "recordings", fmt.Sprintf("%v.wav", uuid.New()))
	if err != nil {
		Log.E("Failed to stop recording:", err)
		return false
	}
	Log.D("Stopped recording. Transcribing...")
	if err := writeWavFile(fp, data); err != nil {
		Log.E("Failed to write WAV file:", err)
		return false
	}
	return true
}
