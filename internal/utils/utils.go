package utils

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/fatih/color"
)

// logger

type LogLevel int

const (
	LevelDebug LogLevel = iota
	LevelInfo
	LevelWarn
	LevelError
	LevelFatal
)

type Logger interface {
	D(format string, args ...interface{})
	I(format string, args ...interface{})
	W(format string, args ...interface{})
	E(format string, args ...interface{})
}

type logger struct {
	level   LogLevel
	appName string
}

func NewLogger(level LogLevel, appName string) Logger {
	return &logger{
		level:   level,
		appName: appName,
	}
}

func (l *logger) logMessage(
	level LogLevel, levelName string, colorFunc func(string, ...interface{}) string,
	format string, args ...interface{},
) {
	if level < l.level {
		return
	}

	timestamp := time.Now().Format("2006-01-02 15:04:05")
	coloredLevel := colorFunc(levelName)
	logLine := fmt.Sprintf(
		"%s | %s | %-10s | %s", timestamp, coloredLevel, l.appName,
		fmt.Sprintf(format, args...),
	)
	fmt.Fprintln(os.Stderr, logLine)
}

func (l *logger) D(format string, args ...interface{}) {
	l.logMessage(LevelDebug, "DEBUG", color.CyanString, format, args...)
}

func (l *logger) I(format string, args ...interface{}) {
	l.logMessage(LevelInfo, "INFO ", color.BlueString, format, args...)
}

func (l *logger) W(format string, args ...interface{}) {
	l.logMessage(LevelWarn, "WARN ", color.YellowString, format, args...)
}

func (l *logger) E(format string, args ...interface{}) {
	l.logMessage(LevelError, "ERROR", color.MagentaString, format, args...)
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
