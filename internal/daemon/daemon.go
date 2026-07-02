package daemon

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/kabilan108/dictator/internal/audio"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/notifier"
	"github.com/kabilan108/dictator/internal/storage"
	"github.com/kabilan108/dictator/internal/typing"
	"github.com/kabilan108/dictator/internal/utils"
	"github.com/kabilan108/dictator/internal/visual"
)

const (
	ErrorDisplayDuration   = 5 * time.Second
	OSDMeterUpdateInterval = time.Second / 30
)

type Daemon struct {
	config      *utils.Config
	recorder    *audio.Recorder
	transcriber audio.WhisperClient
	notifier    notifier.Notifier
	visualSink  visual.Sink
	typer       typing.Typer
	ipcServer   *ipc.Server
	db          *storage.DB

	mu                sync.RWMutex
	state             ipc.DaemonState
	lastError         *string
	recordingDuration time.Duration
	startTime         time.Time
	stopChan          chan struct{}

	osdMeterCh     chan audio.LevelSample
	osdMeterCancel context.CancelFunc
	osdMeterWG     sync.WaitGroup

	operationCtx    context.Context
	operationCancel context.CancelFunc
}

func NewDaemon(cfg *utils.Config) (*Daemon, error) {
	recorder, err := audio.NewRecorder(cfg.Audio)
	if err != nil {
		return nil, fmt.Errorf("failed to create recorder: %w", err)
	}

	transcriber := audio.NewWhisperClient(&cfg.API)

	var stateNotifier notifier.Notifier = notifier.NoopNotifier{}
	if cfg.Notifications != utils.NotificationModeOff {
		stateNotifier, err = notifier.New()
		if err != nil {
			return nil, fmt.Errorf("failed to create notifier: %w", err)
		}
	}

	typer, err := typing.New()
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
		notifier:    stateNotifier,
		visualSink:  visual.NoopSink{},
		typer:       typer,
		db:          db,
		state:       ipc.StateIdle,
		startTime:   time.Now(),
		stopChan:    make(chan struct{}),
	}

	daemon.ipcServer = ipc.NewServer(daemon)

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

	if err := d.updateNotificationState(d.state); err != nil {
		return fmt.Errorf("failed to show initial notification: %w", err)
	}

	d.startOSD()

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

	if d.visualSink != nil {
		d.stopOSDMeterPublisher()
		if err := d.visualSink.Close(); err != nil {
			slog.Error("failed to close OSD sink", "err", err)
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

	if d.state == ipc.StateRecording {
		d.mu.Unlock()
		return fmt.Errorf(ipc.ErrAlreadyRecording)
	}
	if d.state != ipc.StateIdle {
		state := d.state
		d.mu.Unlock()
		return fmt.Errorf("cannot start in current state: %s", state.String())
	}

	slog.Debug("starting recording")

	d.operationCtx, d.operationCancel = context.WithCancel(context.Background())

	if err := d.recorder.Start(); err != nil {
		slog.Error("failed to start recording", "err", err)
		d.mu.Unlock()
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return fmt.Errorf("%s: %w", ipc.ErrRecordingFailed, err)
	}

	d.state = ipc.StateRecording
	d.lastError = nil
	d.recordingDuration = 0
	d.mu.Unlock()

	if err := d.updateNotificationState(ipc.StateRecording); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	duration := time.Duration(0)
	d.publishOSDState(ipc.StateRecording, &duration, "")

	slog.Info("recording started")
	return nil
}

func (d *Daemon) HandleStop() error {
	d.mu.Lock()

	if d.state != ipc.StateRecording {
		d.mu.Unlock()
		return fmt.Errorf(ipc.ErrNotRecording)
	}

	slog.Info("stopping recording and starting transcription")

	recordingDuration := d.recorder.GetRecordingDuration()

	d.state = ipc.StateTranscribing
	d.recordingDuration = recordingDuration
	d.mu.Unlock()

	if err := d.updateNotificationState(ipc.StateTranscribing); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	d.publishOSDState(ipc.StateTranscribing, &recordingDuration, "")

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

	slog.Debug("canceling current operation")

	if d.operationCancel != nil {
		d.operationCancel()
	}

	wasRecording := d.state == ipc.StateRecording

	d.state = ipc.StateIdle
	d.lastError = nil
	d.recordingDuration = 0
	d.mu.Unlock()

	if wasRecording {
		if _, _, err := d.recorder.Stop(); err != nil {
			slog.Error("failed to stop recording during cancel", "err", err)
		}
	}

	if err := d.updateNotificationState(ipc.StateIdle); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	d.publishOSDState(ipc.StateIdle, nil, "")

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

func (d *Daemon) osdSnapshot() visual.StateEvent {
	d.mu.RLock()
	state := d.state
	recordingDuration := d.recordingDuration
	errorMessage := ""
	if d.lastError != nil {
		errorMessage = *d.lastError
	}
	d.mu.RUnlock()

	if state == ipc.StateRecording {
		recordingDuration = d.recorder.GetRecordingDuration()
	}

	return d.osdStateEvent(state, recordingDuration, errorMessage)
}

func (d *Daemon) publishOSDState(state ipc.DaemonState, recordingDuration *time.Duration, errorMessage string) {
	var duration time.Duration
	if recordingDuration != nil {
		duration = *recordingDuration
	}

	d.visualSink.Publish(d.osdStateEvent(state, duration, errorMessage))
}

func publicOSDErrorMessage(errorMessage string) string {
	switch {
	case strings.HasPrefix(errorMessage, ipc.ErrRecordingFailed):
		return ipc.ErrRecordingFailed
	case strings.HasPrefix(errorMessage, ipc.ErrTranscriptionFailed):
		return ipc.ErrTranscriptionFailed
	case strings.HasPrefix(errorMessage, ipc.ErrTypingFailed):
		return ipc.ErrTypingFailed
	default:
		return "dictation failed"
	}
}

func (d *Daemon) osdStateEvent(state ipc.DaemonState, recordingDuration time.Duration, errorMessage string) visual.StateEvent {
	switch state {
	case ipc.StateRecording:
		return visual.NewStateEvent(visual.StateRecording, &recordingDuration, "")
	case ipc.StateTranscribing:
		return visual.NewStateEvent(visual.StateTranscribing, &recordingDuration, "")
	case ipc.StateTyping:
		return visual.NewStateEvent(visual.StateTyping, nil, "")
	case ipc.StateError:
		return visual.NewStateEvent(visual.StateError, nil, publicOSDErrorMessage(errorMessage))
	default:
		return visual.NewStateEvent(visual.StateIdle, nil, "")
	}
}

func (d *Daemon) startOSD() {
	if !d.config.EnableOSD {
		return
	}

	visualSink, err := visual.NewSocketSink(d.osdSnapshot)
	if err != nil {
		slog.Warn("failed to start OSD event socket; continuing without OSD", "err", err)
		return
	}

	d.visualSink = visualSink
	d.startOSDMeterPublisher()
	d.recorder.SetLevelObserver(d.enqueueLevelSample, OSDMeterUpdateInterval)
}

func (d *Daemon) startOSDMeterPublisher() {
	ctx, cancel := context.WithCancel(context.Background())
	d.osdMeterCancel = cancel
	d.osdMeterCh = make(chan audio.LevelSample, 1)
	d.osdMeterWG.Add(1)
	go func() {
		defer d.osdMeterWG.Done()
		for {
			select {
			case <-ctx.Done():
				return
			case sample := <-d.osdMeterCh:
				d.visualSink.Publish(visual.NewMeterEvent(sample.RMS, sample.Peak))
			}
		}
	}()
}

func (d *Daemon) stopOSDMeterPublisher() {
	d.recorder.SetLevelObserver(nil, 0)
	if d.osdMeterCancel != nil {
		d.osdMeterCancel()
		d.osdMeterCancel = nil
	}
	d.osdMeterWG.Wait()
}

func (d *Daemon) enqueueLevelSample(sample audio.LevelSample) {
	d.mu.RLock()
	recording := d.state == ipc.StateRecording
	d.mu.RUnlock()

	if !recording {
		return
	}

	select {
	case d.osdMeterCh <- sample:
		return
	default:
	}

	select {
	case <-d.osdMeterCh:
	default:
	}
	select {
	case d.osdMeterCh <- sample:
	default:
	}
}

func (d *Daemon) transcribeAndType() {
	recordingDuration := d.recorder.GetRecordingDuration()

	audioData, audioPath, err := d.saveRecording()
	if err != nil {
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrRecordingFailed, err))
		return
	}

	d.mu.RLock()
	ctx := d.operationCtx
	d.mu.RUnlock()

	text, err := d.transcribe(ctx, audioData, audioPath)
	if err != nil {
		if ctx.Err() != nil {
			slog.Debug("transcription cancelled")
			return
		}
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTranscriptionFailed, err))
		return
	}

	if err := d.typeAndSave(ctx, text, recordingDuration, audioPath); err != nil {
		return
	}

	d.mu.Lock()
	d.state = ipc.StateIdle
	d.lastError = nil
	d.recordingDuration = 0
	d.mu.Unlock()

	if err := d.updateNotificationState(ipc.StateIdle); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	d.publishOSDState(ipc.StateIdle, nil, "")
}

func (d *Daemon) saveRecording() ([]byte, string, error) {
	audioData, audioPath, err := d.recorder.Stop()
	if err != nil {
		slog.Error("failed to stop recording", "err", err)
		return nil, "", err
	}

	audioFile, err := audio.WriteAudioData(audioPath, audioData)
	if err != nil {
		slog.Error("failed to write audio file", "err", err)
		return nil, "", err
	}
	audioFile.Close()

	slog.Info("audio saved", "filepath", audioPath)
	return audioData, audioPath, nil
}

func (d *Daemon) transcribe(ctx context.Context, audioData []byte, audioPath string) (string, error) {
	activeProvider := d.config.API.Providers[d.config.API.ActiveProvider]
	req := audio.TranscriptionRequest{
		AudioData: audioData,
		Filename:  audioPath,
		Model:     activeProvider.Model,
	}

	resp, err := d.transcriber.Transcribe(ctx, &req)
	if err != nil {
		slog.Error("transcription failed", "err", err)
		return "", err
	}

	slog.Info("transcription complete")
	return resp.Text, nil
}

func (d *Daemon) typeAndSave(ctx context.Context, text string, duration time.Duration, audioPath string) error {
	d.mu.Lock()
	d.state = ipc.StateTyping
	d.mu.Unlock()

	if err := d.updateNotificationState(ipc.StateTyping); err != nil {
		slog.Warn("failed to update notification", "err", err)
	}

	d.publishOSDState(ipc.StateTyping, nil, "")

	if err := d.typer.Type(ctx, text); err != nil {
		if ctx.Err() != nil {
			slog.Debug("typing cancelled")
			d.mu.Lock()
			d.state = ipc.StateIdle
			d.lastError = nil
			d.recordingDuration = 0
			d.mu.Unlock()
			return nil
		}
		slog.Error("typing failed", "err", err)
		d.handleError(fmt.Sprintf("%s: %v", ipc.ErrTypingFailed, err))
		return err
	}

	slog.Info("typing complete")

	activeProvider := d.config.API.Providers[d.config.API.ActiveProvider]
	durationMs := int(duration.Milliseconds())
	if err := d.db.SaveTranscript(durationMs, text, audioPath, activeProvider.Model); err != nil {
		slog.Warn("failed to save transcript to database", "err", err)
	} else {
		slog.Debug("transcript saved to database")
	}

	return nil
}

func (d *Daemon) handleError(errorMsg string) {
	d.mu.Lock()

	d.state = ipc.StateError
	d.lastError = &errorMsg
	d.mu.Unlock()

	if err := d.updateNotificationState(ipc.StateError); err != nil {
		slog.Warn("failed to update error notification", "err", err)
	}

	d.publishOSDState(ipc.StateError, nil, errorMsg)

	// auto-return to idle after error display
	time.AfterFunc(ErrorDisplayDuration, func() {
		d.mu.Lock()
		if d.state != ipc.StateError {
			d.mu.Unlock()
			return
		}
		d.state = ipc.StateIdle
		d.recordingDuration = 0
		d.mu.Unlock()

		if err := d.updateNotificationState(ipc.StateIdle); err != nil {
			slog.Warn("failed to update notification after error", "err", err)
		}

		d.publishOSDState(ipc.StateIdle, nil, "")
	})
}

func (d *Daemon) updateNotificationState(state ipc.DaemonState) error {
	switch d.config.Notifications {
	case utils.NotificationModeAll:
		return d.notifier.UpdateState(state)
	case utils.NotificationModeErrorsOnly:
		if state == ipc.StateError {
			return d.notifier.UpdateState(state)
		}
		return nil
	case utils.NotificationModeOff:
		return nil
	default:
		return fmt.Errorf("unknown notification mode: %s", d.config.Notifications)
	}
}

func NotRunning(e error) error {
	if e != nil {
		return fmt.Errorf("can't connect to daemon: %v", e)
	}
	return nil
}
