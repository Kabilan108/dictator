package daemon

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"github.com/kabilan108/dictator/internal/audio"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/notifier"
	"github.com/kabilan108/dictator/internal/typing"
	"github.com/kabilan108/dictator/internal/utils"
)

type Daemon struct {
	config      *utils.Config
	recorder    *audio.Recorder
	transcriber audio.WhisperClient
	notifier    notifier.Notifier
	typer       typing.Typer
	ipcServer   *ipc.Server
	log         utils.Logger

	mu        sync.RWMutex
	state     ipc.DaemonState
	lastError *string
	startTime time.Time
	stopChan  chan struct{}

	operationCtx    context.Context
	operationCancel context.CancelFunc

	notificationTimer *time.Timer
}

func NewDaemon(cfg *utils.Config) (*Daemon, error) {
	log := utils.NewLogger(cfg.App.LogLevel, "daemon")

	recorder, err := audio.NewRecorder(cfg.Audio, cfg.App.LogLevel)
	if err != nil {
		return nil, fmt.Errorf("failed to create recorder: %w", err)
	}

	transcriber := audio.NewWhisperClient(&cfg.API, cfg.App.LogLevel)

	notifier, err := notifier.New(cfg.App.LogLevel)
	if err != nil {
		return nil, fmt.Errorf("failed to create notifier: %w", err)
	}

	typer := typing.New(cfg.App.LogLevel)

	daemon := &Daemon{
		config:      cfg,
		recorder:    recorder,
		transcriber: transcriber,
		notifier:    notifier,
		typer:       typer,
		log:         log,
		state:       ipc.StateIdle,
		startTime:   time.Now(),
		stopChan:    make(chan struct{}),
	}

	daemon.ipcServer = ipc.NewServer(daemon, cfg.App.LogLevel)

	return daemon, nil
}

func (d *Daemon) Run() error {
	d.log.D("starting dictator daemon...")

	if err := d.ipcServer.Start(); err != nil {
		return fmt.Errorf("failed to start IPC server: %w", err)
	}
	defer func() {
		if err := d.ipcServer.Stop(); err != nil {
			d.log.E("failed to stop IPC server: %v", err)
		}
	}()

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to show initial notification: %v", err)
	}

	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	d.log.I("dictator daemon started successfully")

	for {
		select {
		case sig := <-sigChan:
			d.log.D("received signal %v, shutting down", sig)
			return d.shutdown()

		case <-d.stopChan:
			d.log.D("daemon stop requested")
			return d.shutdown()
		}
	}
}

func (d *Daemon) Stop() {
	select {
	case d.stopChan <- struct{}{}:
	default:
		// channel is already closed or full
	}
}

func (d *Daemon) shutdown() error {
	d.log.D("shutting down daemon...")

	d.mu.Lock()
	d.stopNotificationTimer()
	if d.operationCancel != nil {
		d.operationCancel()
	}
	d.mu.Unlock()

	var lastErr error

	if d.recorder != nil {
		if err := d.recorder.Close(); err != nil {
			d.log.E("failed to close recorder: %v", err)
			lastErr = err
		}
	}

	if d.notifier != nil {
		if err := d.notifier.Close(); err != nil {
			d.log.E("failed to close notifier: %v", err)
			lastErr = err
		}
	}

	close(d.stopChan)
	d.log.I("daemon shutdown complete")

	return lastErr
}

// implement CommandHandler interface

func (d *Daemon) HandleStart() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.state == ipc.StateRecording {
		return fmt.Errorf(ipc.ErrAlreadyRecording)
	}

	d.log.D("starting recording...")

	d.operationCtx, d.operationCancel = context.WithCancel(context.Background())

	if err := d.recorder.Start(); err != nil {
		d.log.E("failed to start recording: %v", err)
		msg := err.Error()
		d.lastError = &msg
		return fmt.Errorf("%s: %w", ipc.ErrRecordingFailed, err)
	}

	d.state = ipc.StateRecording
	d.lastError = nil

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update notification: %v", err)
	}

	d.startNotificationTimer()

	d.log.I("recording started")
	return nil
}

func (d *Daemon) HandleStop() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.state != ipc.StateRecording {
		return fmt.Errorf(ipc.ErrNotRecording)
	}

	d.log.I("stopping recording and starting transcription...")

	d.stopNotificationTimer()

	d.state = ipc.StateTranscribing
	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update notification: %v", err)
	}

	go d.transcribeAndType()

	return nil
}

func (d *Daemon) HandleToggle() error {
	d.mu.RLock()
	currentState := d.state
	d.mu.RUnlock()

	switch currentState {
	case ipc.StateIdle:
		return d.HandleStart()
	case ipc.StateRecording:
		return d.HandleStop()
	default:
		return fmt.Errorf("cannot toggle in current state: %s", currentState.String())
	}
}

func (d *Daemon) HandleCancel() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	d.log.D("canceling current operation...")

	d.stopNotificationTimer()

	if d.operationCancel != nil {
		d.operationCancel()
	}

	if d.state == ipc.StateRecording {
		if _, _, err := d.recorder.Stop(); err != nil {
			d.log.E("failed to stop recording during cancel: %v", err)
		}
	}

	d.state = ipc.StateIdle
	d.lastError = nil

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update notification: %v", err)
	}

	d.log.I("operation canceled")
	return nil
}

func (d *Daemon) GetStatus() ipc.StatusData {
	d.mu.RLock()
	defer d.mu.RUnlock()

	status := ipc.StatusData{
		State:  d.state,
		Uptime: time.Since(d.startTime),
	}

	if d.state == ipc.StateRecording {
		duration := d.recorder.GetRecordingDuration()
		status.RecordingDuration = &duration
	}

	if d.lastError != nil {
		status.LastError = d.lastError
	}

	return status
}

func (d *Daemon) transcribeAndType() {
	audioData, audioPath, err := d.recorder.Stop()
	if err != nil {
		d.log.E("failed to stop recording: %v", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return
	}

	audioFile, err := audio.WriteAudioData(audioPath, audioData)
	if err != nil {
		d.log.E("failed to write audio file: %v", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return
	}
	defer audioFile.Close()

	d.log.I("audio saved to %s", audioPath)

	req := audio.TranscriptionRequest{
		AudioData: audioData,
		Filename:  audioFile.Name(),
		Model:     d.config.API.Model,
	}

	d.mu.RLock()
	ctx := d.operationCtx
	d.mu.RUnlock()

	resp, err := d.transcriber.Transcribe(ctx, &req)
	if err != nil {
		d.log.E("transcription failed: %v", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTranscriptionFailed, err))
		return
	}

	d.log.I("transcription complete")

	d.mu.Lock()
	d.state = ipc.StateTyping
	d.mu.Unlock()

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update notification: %v", err)
	}

	if err := d.typer.TypeText(ctx, resp.Text); err != nil {
		if ctx.Err() != nil {
			d.log.I("typing cancelled")
			d.mu.Lock()
			d.state = ipc.StateIdle
			d.lastError = nil
			d.mu.Unlock()
			return
		}
		d.log.E("typing failed: %v", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTypingFailed, err))
		return
	}

	d.log.I("typing complete")

	d.mu.Lock()
	d.state = ipc.StateIdle
	d.lastError = nil
	d.mu.Unlock()

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update notification: %v", err)
	}
}

func (d *Daemon) startNotificationTimer() {
	d.stopNotificationTimer()
	d.notificationTimer = time.AfterFunc(1*time.Second, func() {
		d.updateRecordingNotification()
	})
}

func (d *Daemon) stopNotificationTimer() {
	if d.notificationTimer != nil {
		d.notificationTimer.Stop()
		d.notificationTimer = nil
	}
}

func (d *Daemon) updateRecordingNotification() {
	d.mu.RLock()
	state := d.state
	d.mu.RUnlock()

	if state == ipc.StateRecording {
		duration := d.recorder.GetRecordingDuration()
		if err := d.notifier.UpdateStateWithDuration(state, duration); err != nil {
			d.log.W("failed to update recording notification: %v", err)
		}

		// Schedule next update
		d.mu.Lock()
		if d.state == ipc.StateRecording {
			d.notificationTimer = time.AfterFunc(1*time.Second, func() {
				d.updateRecordingNotification()
			})
		}
		d.mu.Unlock()
	}
}

func (d *Daemon) handleError(errorMsg string) {
	d.mu.Lock()
	defer d.mu.Unlock()

	d.stopNotificationTimer()

	d.state = ipc.StateError
	d.lastError = &errorMsg

	if err := d.notifier.UpdateState(d.state); err != nil {
		d.log.W("failed to update error notification: %v", err)
	}

	// auto-return to idle after error display
	time.AfterFunc(5*time.Second, func() {
		d.mu.Lock()
		d.state = ipc.StateIdle
		d.mu.Unlock()

		if err := d.notifier.UpdateState(d.state); err != nil {
			d.log.W("failed to update notification after error: %v", err)
		}
	})
}
