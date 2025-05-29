package typing

import (
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/kabilan108/dictator/internal/utils"
)

type Typer interface {
	TypeText(ctx context.Context, text string) error
	IsAvailable() bool
}

func New(logLevel utils.LogLevel) Typer {
	log := utils.NewLogger(logLevel, "typing")

	xdotoolTyper := &XdotoolTyper{
		config: utils.AppConfig{},
		log:    log,
	}
	if xdotoolTyper.IsAvailable() {
		log.D("using xdotool for text input")
		return xdotoolTyper
	}

	// Fallback to xclip
	xclipTyper := &XclipTyper{
		config: utils.AppConfig{},
		log:    log,
	}
	if xclipTyper.IsAvailable() {
		log.W("xdotool not available, falling back to xclip (clipboard)")
		return xclipTyper
	}

	// Return xdotool typer even if not available - it will fail gracefully
	log.W("neither xdotool nor xclip available - typing may fail")
	return xdotoolTyper
}

type XdotoolTyper struct {
	config utils.AppConfig
	log    utils.Logger
}

func (x *XdotoolTyper) IsAvailable() bool {
	_, err := exec.LookPath("xdotool")
	return err == nil
}

func (x *XdotoolTyper) TypeText(ctx context.Context, text string) error {
	if text == "" {
		x.log.W("empty text provided, nothing to type")
		return nil
	}

	cmd := exec.CommandContext(ctx, "xdotool", "type", "--clearmodifiers", "--", text)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			x.log.I("typing cancelled by context")
			return ctx.Err()
		}
		x.log.E("xdotool command failed: %v", err)
		return fmt.Errorf("failed to type text with xdotool: %w", err)
	}

	// Apply typing delay if configured, but check for cancellation
	if x.config.TypingDelayMS > 0 {
		delay := time.Duration(x.config.TypingDelayMS) * time.Millisecond
		x.log.D("applying typing delay: %v", delay)
		
		select {
		case <-time.After(delay):
			// Normal delay completion
		case <-ctx.Done():
			x.log.I("typing cancelled during delay")
			return ctx.Err()
		}
	}

	x.log.I("successfully typed %d characters", len(text))
	return nil
}

type XclipTyper struct {
	config utils.AppConfig
	log    utils.Logger
}

func (x *XclipTyper) IsAvailable() bool {
	_, err := exec.LookPath("xclip")
	return err == nil
}

func (x *XclipTyper) TypeText(ctx context.Context, text string) error {
	if text == "" {
		x.log.W("empty text provided, nothing to copy")
		return nil
	}

	cmd := exec.CommandContext(ctx, "xclip", "-selection", "clipboard")
	cmd.Stdin = strings.NewReader(text)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			x.log.I("clipboard operation cancelled by context")
			return ctx.Err()
		}
		x.log.E("xclip command failed: %v", err)
		return fmt.Errorf("failed to copy text to clipboard with xclip: %w", err)
	}

	x.log.I("text copied to clipboard (%d characters) - paste with Ctrl+V", len(text))
	return nil
}

