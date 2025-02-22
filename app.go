package main

// TODO: make this better for longer recordings
//       when the recording > THRESHOLD sec long -> split and start
//       transcribing each part

// TODO: run a job on start up that will remove old recordings

import (
	"context"
	"fmt"

	"dictator/app"
)

// App struct
type App struct {
	ctx context.Context
	ar  *app.AudioRecorder
	ws  *app.WhisperServer
}

type Result struct {
	Success    bool   `json:"success"`
	Transcript string `json:"transcript,omitempty"`
	Error      string `json:"error,omitempty"`
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

	ws, err := app.NewWhisperServer()
	if err != nil {
		panic(fmt.Sprintf("failed to start whisper server: %v", err))
	}

	err = ws.Start()
	if err != nil {
		panic(fmt.Sprintf("failed to start whisper server: %v", err))
	}

	a.ctx = ctx
	a.ar = ar
	a.ws = ws
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
	if err := a.ar.Terminate(); err != nil {
		app.Log.E("Failed to terminate audio recorder:", err)
	}
	if err := a.ws.Stop(); err != nil {
		app.Log.E("Failed to stop whisper server:", err)
	}
	app.Log.I("Successfully cleaned up resources.")
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

	app.Log.D("Stopped recording. Saving WAV file.")
	if err := app.WriteWavFile(fp, data); err != nil {
		app.Log.E("Failed to write WAV file:", err)
		return Result{Success: false, Error: "Failed to write wav file"}
	}

	app.Log.D("Transcribing audio...")
	transcript, err := a.ws.Transcribe(fp)
	if err != nil {
		app.Log.E("Failed to transcribe audio:", err)
		return Result{Success: false, Error: "Failed to transcribe audio"}
	}

	app.Log.D("Transcription complete: %s", transcript)
	return Result{Success: true, Transcript: transcript}
}
