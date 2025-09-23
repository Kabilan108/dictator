package daemon

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"github.com/kabilan108/dictator/internal/audio"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/notifier"
	"github.com/kabilan108/dictator/internal/storage"
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
	db          *storage.DB

	mu        sync.RWMutex
	state     ipc.DaemonState
	lastError *string
	startTime time.Time
	stopChan  chan struct{}

	operationCtx    context.Context
	operationCancel context.CancelFunc

	notificationTimer *time.Timer
}

func NewDaemon(cfg *utils.Config, logLevel string) (*Daemon, error) {
	recorder, err := audio.NewRecorder(cfg.Audio, logLevel)
	if err != nil {
		return nil, fmt.Errorf("failed to create recorder: %w", err)
	}

	transcriber := audio.NewWhisperClient(&cfg.API, logLevel)

	notifier, err := notifier.New(logLevel)
	if err != nil {
		return nil, fmt.Errorf("failed to create notifier: %w", err)
	}

	typer, err := typing.New(logLevel)
	if err != nil {
		return nil, fmt.Errorf("failed to create typer: %w", err)
	}

	db, err := storage.NewDB()
	if err != nil {
		return nil, fmt.Errorf("failed to create database: %w", err)
	}

	daemon := &Daemon{
		config:      cfg,
		recorder:    recorder,
		transcriber: transcriber,
		notifier:    notifier,
		typer:       typer,
		db:          db,
		state:       ipc.StateIdle,
		startTime:   time.Now(),
		stopChan:    make(chan struct{}),
	}

	daemon.ipcServer = ipc.NewServer(daemon, logLevel)

	return daemon, nil
}

func (d *Daemon) Run() error {
	slog.Debug("starting dictator daemon")

	if err := d.ipcServer.Start(); err != nil {
		return fmt.Errorf("failed to start IPC server: %w", err)
	}
	defer func() {
		if err := d.ipcServer.Stop(); err != nil {
			slog.Error("failed to stop IPC server", "err", err)
		}
	}()

	if err := d.notifier.UpdateState(d.state); err != nil {
		return fmt.Errorf("failed to show initial notification: %w", err)
	}

	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	slog.Info("dictator daemon started successfully")

	for {
		select {
		case sig := <-sigChan:
			slog.Debug("received signal", "signal", sig)
			return d.shutdown()

		case <-d.stopChan:
			slog.Debug("daemon stop requested")
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
	slog.Debug("shutting down daemon")

	d.mu.Lock()
	d.stopNotificationTimer()
	if d.operationCancel != nil {
		d.operationCancel()
	}
	d.mu.Unlock()

	var lastErr error

	if d.recorder != nil {
		if err := d.recorder.Close(); err != nil {
			slog.Error("failed to close recorder", "err", err)
			lastErr = err
		}
	}

	if d.notifier != nil {
		if err := d.notifier.Close(); err != nil {
			slog.Error("failed to close notifier", "err", err)
			lastErr = err
		}
	}

	if d.db != nil {
		if err := d.db.Close(); err != nil {
			slog.Error("failed to close database", "err", err)
			lastErr = err
		}
	}

	close(d.stopChan)
	slog.Info("daemon shutdown complete")

	return lastErr
}

// implement CommandHandler interface

func (d *Daemon) HandleStart() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.state == ipc.StateRecording {
		return fmt.Errorf(ipc.ErrAlreadyRecording)
	}

	slog.Debug("starting recording")

	d.operationCtx, d.operationCancel = context.WithCancel(context.Background())

	if err := d.recorder.Start(); err != nil {
		slog.Error("failed to start recording", "err", err)
		msg := err.Error()
		d.lastError = &msg
		return fmt.Errorf("%s: %w", ipc.ErrRecordingFailed, err)
	}

	d.state = ipc.StateRecording
	d.lastError = nil

	if err := d.notifier.UpdateState(d.state); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	d.startNotificationTimer()

	slog.Info("recording started")
	return nil
}

func (d *Daemon) HandleStop() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.state != ipc.StateRecording {
		return fmt.Errorf(ipc.ErrNotRecording)
	}

	slog.Info("stopping recording and starting transcription")

	d.stopNotificationTimer()

	d.state = ipc.StateTranscribing
	if err := d.notifier.UpdateState(d.state); err != nil {
		slog.Warn("failed to update notification", "err", err)
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

	slog.Debug("canceling current operation")

	d.stopNotificationTimer()

	if d.operationCancel != nil {
		d.operationCancel()
	}

	if d.state == ipc.StateRecording {
		if _, _, err := d.recorder.Stop(); err != nil {
			slog.Error("failed to stop recording during cancel", "err", err)
		}
	}

	d.state = ipc.StateIdle
	d.lastError = nil

	if err := d.notifier.UpdateState(d.state); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	slog.Info("operation canceled")
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
	recordingDuration := d.recorder.GetRecordingDuration()

	audioData, audioPath, err := d.recorder.Stop()
	if err != nil {
		slog.Error("failed to stop recording", "err", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return
	}

	audioFile, err := audio.WriteAudioData(audioPath, audioData)
	if err != nil {
		slog.Error("failed to write audio file", "err", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return
	}
	defer audioFile.Close()

	slog.Info("audio saved", "filepath", audioPath)

	activeProvider := d.config.API.Providers[d.config.API.ActiveProvider]
	req := audio.TranscriptionRequest{
		AudioData: audioData,
		Filename:  audioFile.Name(),
		Model:     activeProvider.Model,
	}

	d.mu.RLock()
	ctx := d.operationCtx
	d.mu.RUnlock()

	resp, err := d.transcriber.Transcribe(ctx, &req)
	if err != nil {
		slog.Error("transcription failed", "err", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTranscriptionFailed, err))
		return
	}

	slog.Info("transcription complete")

	d.mu.Lock()
	d.state = ipc.StateTyping
	d.mu.Unlock()

	if err := d.notifier.UpdateState(d.state); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	if err := d.typer.TypeText(ctx, resp.Text); err != nil {
		if ctx.Err() != nil {
			slog.Debug("typing cancelled")
			d.mu.Lock()
			d.state = ipc.StateIdle
			d.lastError = nil
			d.mu.Unlock()
		} else {
			slog.Error("typing failed", "err", err)
			d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTypingFailed, err))
			return
		}
	}

	slog.Info("typing complete")

	durationMs := int(recordingDuration.Milliseconds())
	if err := d.db.SaveTranscript(durationMs, resp.Text, audioPath, activeProvider.Model); err != nil {
		slog.Warn("failed to save transcript to database", "err", err)
	} else {
		slog.Debug("transcript saved to database")
	}

	d.mu.Lock()
	d.state = ipc.StateIdle
	d.lastError = nil
	d.mu.Unlock()

	if err := d.notifier.UpdateState(d.state); err != nil {
		slog.Warn("failed to update notification", "err", err)
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
			slog.Warn("failed to update recording notification", "err", err)
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
		slog.Warn("failed to update error notification", "err", err)
	}

	// auto-return to idle after error display
	time.AfterFunc(5*time.Second, func() {
		d.mu.Lock()
		d.state = ipc.StateIdle
		d.mu.Unlock()

		if err := d.notifier.UpdateState(d.state); err != nil {
			slog.Warn("failed to update notification after error", "err", err)
		}
	})
}

func NotRunning(e error) error {
	return fmt.Errorf("can't connect to daemon: %v", e)
}
