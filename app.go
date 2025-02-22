package main

// TODO: make this better for longer recordings
//       when the recording > THRESHOLD sec long -> split and start
//       transcribing each part

import (
	"context"
	"fmt"

	"dictator/app"
)

// App struct
type App struct {
	ctx context.Context
	ar  *app.AudioRecorder
}

type Result struct {
	Success bool   `json:"success"`
	Error   string `json:"error,omitempty"`
}

// NewApp creates a new App application struct
func NewApp() *App {
	return &App{}
}

// startup is called at application startup
func (a *App) startup(ctx context.Context) {
	ar, err := app.NewAudioRecorder()
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
func (a *App) StartRecording() Result {
	if err := a.ar.StartRecording(); err != nil {
		app.Log.E("Failed to start recording:", err)
		return Result{Success: false, Error: "Failed to start recording"}
	}
	app.Log.D("Started recording")
	return Result{Success: true}
}

// stop a recording
func (a *App) StopRecording() Result {
	data, err := a.ar.StopRecording()
	if err != nil {
		app.Log.E("Failed to stop recording:", err)
		return Result{Success: false, Error: "Failed to stop recording"}
	}

	fp, err := app.NewRecordingFile()
	if err != nil {
		app.Log.E("Failed to create wav file:", err)
		return Result{Success: false, Error: "Failed to create wav file"}
	}

	app.Log.D("Stopped recording. Transcribing...")
	if err := app.WriteWavFile(fp, data); err != nil {
		app.Log.E("Failed to write WAV file:", err)
		return Result{Success: false, Error: "Failed to write wav file"}
	}

	return Result{Success: true}
}
