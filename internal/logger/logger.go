package logger

import (
	"fmt"
	"os"
	"time"

	"github.com/fatih/color"
)

type LogLevel int

const (
	DEBUG LogLevel = iota
	INFO
	WARN
	ERROR
	FATAL
)

type Logger interface {
	D(message string)
	I(message string)
	W(message string)
	E(message string)
	F(message string)
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

func (l *logger) logMessage(level LogLevel, levelName string, colorFunc func(string, ...interface{}) string, message string) {
	if level < l.level {
		return
	}

	timestamp := time.Now().Format("2006-01-02 15:04:05")
	coloredLevel := colorFunc(levelName)
	logLine := fmt.Sprintf("%s | %s | %-10s | %s", timestamp, coloredLevel, l.appName, message)

	fmt.Fprintln(os.Stderr, logLine)
}

func (l *logger) D(message string) {
	l.logMessage(DEBUG, "DEBUG", color.CyanString, message)
}

func (l *logger) I(message string) {
	l.logMessage(INFO, "INFO ", color.BlueString, message)
}

func (l *logger) W(message string) {
	l.logMessage(WARN, "WARN ", color.YellowString, message)
}

func (l *logger) E(message string) {
	l.logMessage(ERROR, "ERROR", color.MagentaString, message)
}

func (l *logger) F(message string) {
	l.logMessage(FATAL, "FATAL", color.RedString, message)
	os.Exit(1)
}
