package utils

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"time"
)

// logger

type MultiHandler struct {
	fileHandler   slog.Handler
	stderrHandler slog.Handler
}

func (h *MultiHandler) Enabled(ctx context.Context, level slog.Level) bool {
	return h.fileHandler.Enabled(ctx, level) || h.stderrHandler.Enabled(ctx, level)
}

func (h *MultiHandler) Handle(ctx context.Context, record slog.Record) error {
	var errs []error

	if err := h.fileHandler.Handle(ctx, record); err != nil {
		errs = append(errs, err)
	}

	if err := h.stderrHandler.Handle(ctx, record); err != nil {
		errs = append(errs, err)
	}

	if len(errs) > 0 {
		return fmt.Errorf("handler errors: %v", errs)
	}

	return nil
}

func (h *MultiHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	return &MultiHandler{
		fileHandler:   h.fileHandler.WithAttrs(attrs),
		stderrHandler: h.stderrHandler.WithAttrs(attrs),
	}
}

func (h *MultiHandler) WithGroup(name string) slog.Handler {
	return &MultiHandler{
		fileHandler:   h.fileHandler.WithGroup(name),
		stderrHandler: h.stderrHandler.WithGroup(name),
	}
}

type Logger struct {
	logFile *os.File
}

var LevelMap = map[string]slog.Level{
	"DEBUG": slog.LevelDebug,
	"INFO":  slog.LevelInfo,
	"WARN":  slog.LevelWarn,
	"ERROR": slog.LevelError,
}

func SetupLogger(level string) *Logger {
	logLevel, exists := LevelMap[level]
	if !exists {
		fmt.Fprintf(os.Stderr, "invalid log level: %s\n", level)
		os.Exit(1)
	}

	logFile, err := os.OpenFile(
		filepath.Join(CACHE_DIR, "app.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o666,
	)
	if err != nil {
		panic(fmt.Errorf("failed to open log file: %w", err))
	}

	fileHandler := slog.NewJSONHandler(logFile, &slog.HandlerOptions{AddSource: true, Level: logLevel})
	stderrHandler := slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: logLevel})
	logHandler := &MultiHandler{fileHandler, stderrHandler}

	slog.SetDefault(slog.New(logHandler))
	return &Logger{logFile}
}

func (l *Logger) Close() {
	if l.logFile != nil {
		l.logFile.Close()
	}
}

// files

type AppDir int

const (
	CacheDir AppDir = iota
	ConfigDir
)

func createDir(path string) error {
	if _, err := os.Stat(path); os.IsNotExist(err) {
		err = os.MkdirAll(path, 0o755)
		if err != nil {
			return fmt.Errorf("unable to create directory: %w", err)
		}
	}
	return nil
}

func CreateAppDir(ad AppDir) func(name string) (string, error) {
	var d string
	switch ad {
	case CacheDir:
		d = CACHE_DIR
	case ConfigDir:
		d = CONFIG_DIR
	}
	return func(name string) (string, error) {
		fp := filepath.Join(d, name)
		if err := createDir(fp); err != nil {
			return "", fmt.Errorf("failed to create directory: %w", err)
		}
		return fp, nil
	}
}

func GetPathToRecording(startTime time.Time) (string, error) {
	d, err := CreateAppDir(CacheDir)("recordings")
	if err != nil {
		return "", fmt.Errorf("failed to create recording directory: %w", err)
	}
	now := startTime.Format("01022006-150405")
	fp := filepath.Join(d, fmt.Sprintf("%v.wav", now))
	return fp, nil
}
