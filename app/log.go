package app

import (
	"fmt"
	"runtime"
	"strings"
	"time"
)

// ANSI color codes
const (
	colorReset  = "\033[0m"
	colorRed    = "\033[31m"
	colorGreen  = "\033[32m"
	colorYellow = "\033[33m"
	colorBlue   = "\033[34m"
	colorPurple = "\033[35m"
	colorCyan   = "\033[36m"
	colorGray   = "\033[37m"
)

type LogLevel int

const (
	DEBUG LogLevel = iota
	INFO
	WARN
	ERROR
)

func (l LogLevel) String() string {
	switch l {
	case DEBUG:
		return colorGray + "DEBUG" + colorReset
	case INFO:
		return colorGreen + "INFO" + colorReset
	case WARN:
		return colorYellow + "WARN" + colorReset
	case ERROR:
		return colorRed + "ERROR" + colorReset
	default:
		return "UNKNOWN"
	}
}

type Logger struct {
	level LogLevel
}

func NewLogger(level LogLevel) *Logger {
	return &Logger{level: level}
}

func (l *Logger) log(level LogLevel, msg string, args ...interface{}) {
	if level < l.level {
		return
	}

	// Get caller info
	_, file, line, _ := runtime.Caller(2)
	parts := strings.Split(file, "/")
	file = parts[len(parts)-1]

	timestamp := time.Now().Format("2006-01-02 15:04:05")

	if len(args) > 0 {
		msg = fmt.Sprintf(msg, args...)
	}

	// Format with colors and consistent spacing
	fmt.Printf("%-23s | %-15s | %-20s | %s\n",
		colorCyan+timestamp+colorReset,
		level.String(),
		colorPurple+fmt.Sprintf("%s:%d", file, line)+colorReset,
		msg)
}

func (l *Logger) D(msg string, args ...interface{}) {
	l.log(DEBUG, msg, args...)
}

func (l *Logger) I(msg string, args ...interface{}) {
	l.log(INFO, msg, args...)
}

func (l *Logger) W(msg string, args ...interface{}) {
	l.log(WARN, msg, args...)
}

func (l *Logger) E(msg string, args ...interface{}) {
	l.log(ERROR, msg, args...)
}

var Log = NewLogger(DEBUG)

func TestLogger() {
	Log.D("Debug message: %s", "test")
	Log.I("Info message")
	Log.W("Warning message with value: %d", 42)
	Log.E("Error occurred")
}
