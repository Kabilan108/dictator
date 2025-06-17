package typing

import (
	"context"
	"fmt"
	"log/slog"
	"os/exec"
	"strings"
	"time"

	"github.com/kabilan108/dictator/internal/utils"
)

type Typer interface {
	TypeText(ctx context.Context, text string) error
	IsAvailable() bool
}

func New(logLevel string) (Typer, error) {
	xdotoolTyper := &XdotoolTyper{config: utils.AppConfig{}}
	if xdotoolTyper.IsAvailable() {
		slog.Debug("using xdotool for text input")
		return xdotoolTyper, nil
	}

	// Fallback to xclip
	xclipTyper := &XclipTyper{Config: utils.AppConfig{}}
	if xclipTyper.IsAvailable() {
		slog.Warn("xdotool not available, falling back to xclip (clipboard)")
		return xclipTyper, nil
	}

	// Return xdotool typer even if not available - it will fail gracefully
	return nil, fmt.Errorf("neither xdotool nor xclip available")
}

type XdotoolTyper struct {
	config utils.AppConfig
}

func (x *XdotoolTyper) IsAvailable() bool {
	_, err := exec.LookPath("xdotool")
	return err == nil
}

func (x *XdotoolTyper) TypeText(ctx context.Context, text string) error {
	if text == "" {
		slog.Debug("empty text provided, nothing to type")
		return nil
	}

	cmd := exec.CommandContext(ctx, "xdotool", "type", "--clearmodifiers", "--", text)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			slog.Debug("typing cancelled by context")
			return ctx.Err()
		}
		slog.Error("xdotool command failed", "err", err)
		return fmt.Errorf("failed to type text with xdotool: %w", err)
	}

	// Apply typing delay if configured, but check for cancellation
	if x.config.TypingDelayMS > 0 {
		delay := time.Duration(x.config.TypingDelayMS) * time.Millisecond
		slog.Debug("applying typing delay", "delay", delay)

		select {
		case <-time.After(delay):
			// Normal delay completion
		case <-ctx.Done():
			slog.Debug("typing cancelled during delay")
			return ctx.Err()
		}
	}

	slog.Debug("successfully typed", "text", text)
	return nil
}

type XclipTyper struct {
	Config utils.AppConfig
	Log    utils.Logger
}

func (x *XclipTyper) IsAvailable() bool {
	_, err := exec.LookPath("xclip")
	return err == nil
}

func (x *XclipTyper) TypeText(ctx context.Context, text string) error {
	if text == "" {
		slog.Debug("empty text provided, nothing to copy")
		return nil
	}

	cmd := exec.CommandContext(ctx, "xclip", "-selection", "clipboard")
	cmd.Stdin = strings.NewReader(text)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			slog.Debug("clipboard operation cancelled by context")
			return ctx.Err()
		}
		slog.Error("xclip command failed", "err", err)
		return fmt.Errorf("failed to copy text to clipboard with xclip: %w", err)
	}

	slog.Debug("text copied to clipboard")
	return nil
}
