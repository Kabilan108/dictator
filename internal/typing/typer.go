package typing

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"strings"
)

type Typer interface {
	Type(ctx context.Context, text string) error
	IsAvailable() bool
}

type StreamingTyper interface {
	Typer
	TypeIncremental(ctx context.Context, newChars string) error
	Backspace(ctx context.Context, count int) error
}

// detects if the current session is running Wayland
func isWayland() bool {
	if sessionType := os.Getenv("XDG_SESSION_TYPE"); sessionType == "wayland" {
		return true
	}
	if waylandDisplay := os.Getenv("WAYLAND_DISPLAY"); waylandDisplay != "" {
		return true
	}
	return false
}

// creates a Typer implementation based on the current display server
func New() (Typer, error) {
	if isWayland() {
		wtype := &WaylandTyper{}
		if wtype.IsAvailable() {
			slog.Debug("using wtype for text input (wayland)")
			return wtype, nil
		}
		return nil, fmt.Errorf("wayland detected but wtype not available (install wtype and wl-copy)")
	}

	typer := &X11Typer{}
	if typer.IsAvailable() {
		slog.Debug("using xclip/xdotool for text input (x11)")
		return typer, nil
	}
	return nil, fmt.Errorf("x11 detected but xclip/xdotool not available")
}

func areInstalled(cmds ...string) bool {
	for _, cmd := range cmds {
		if _, err := exec.LookPath(cmd); err != nil {
			return false
		}
	}
	return true
}

func copyAndPaste(ctx context.Context, text string, copyCmd, pasteCmd []string) error {
	if text == "" {
		slog.Debug("empty text provided, nothing to type")
		return nil
	}

	cmd := exec.CommandContext(ctx, copyCmd[0], copyCmd[1:]...)
	cmd.Stdin = strings.NewReader(text)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			slog.Debug("clipboard operation cancelled by context")
			return ctx.Err()
		}
		return fmt.Errorf("failed to copy text to clipboard: %w", err)
	}

	slog.Debug("text copied to clipboard")

	cmd = exec.CommandContext(ctx, pasteCmd[0], pasteCmd[1:]...)

	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			slog.Debug("paste operation cancelled by context")
			return ctx.Err()
		}
		return fmt.Errorf("failed to paste: %w", err)
	}

	slog.Debug("typing successful")
	return nil
}

type X11Typer struct{}

func (x *X11Typer) IsAvailable() bool { return areInstalled("xclip", "xdotool") }

func (x *X11Typer) Type(ctx context.Context, text string) error {
	return copyAndPaste(ctx, text,
		[]string{"xclip", "-selection", "clipboard"},
		[]string{"xdotool", "key", "ctrl+shift+v"},
	)
}

type WaylandTyper struct{}

func (w *WaylandTyper) IsAvailable() bool { return areInstalled("wl-copy", "wtype") }

func (w *WaylandTyper) Type(ctx context.Context, text string) error {
	return copyAndPaste(ctx, text,
		[]string{"wl-copy"},
		[]string{"wtype", "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "ctrl", "-m", "shift"},
	)
}

func (w *WaylandTyper) TypeIncremental(ctx context.Context, newChars string) error {
	return w.Type(ctx, newChars)
}

func (w *WaylandTyper) Backspace(ctx context.Context, count int) error {
	if count <= 0 {
		return nil
	}
	for i := 0; i < count; i++ {
		cmd := exec.CommandContext(ctx, "wtype", "-k", "BackSpace")
		if err := cmd.Run(); err != nil {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			return fmt.Errorf("wtype backspace failed: %w", err)
		}
	}
	return nil
}

type YdotoolTyper struct{}

func (y *YdotoolTyper) IsAvailable() bool { return areInstalled("ydotool") }

func (y *YdotoolTyper) Type(ctx context.Context, text string) error {
	if text == "" {
		return nil
	}
	cmd := exec.CommandContext(ctx, "ydotool", "type", "--", text)
	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return fmt.Errorf("ydotool type failed: %w", err)
	}
	return nil
}

func (y *YdotoolTyper) TypeIncremental(ctx context.Context, newChars string) error {
	return y.Type(ctx, newChars)
}

func (y *YdotoolTyper) Backspace(ctx context.Context, count int) error {
	if count <= 0 {
		return nil
	}
	for i := 0; i < count; i++ {
		cmd := exec.CommandContext(ctx, "ydotool", "key", "14:1", "14:0")
		if err := cmd.Run(); err != nil {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			return fmt.Errorf("ydotool backspace failed: %w", err)
		}
	}
	return nil
}

func (x *X11Typer) TypeIncremental(ctx context.Context, newChars string) error {
	return x.Type(ctx, newChars)
}

func (x *X11Typer) Backspace(ctx context.Context, count int) error {
	if count <= 0 {
		return nil
	}
	keys := make([]string, count)
	for i := range keys {
		keys[i] = "BackSpace"
	}
	cmd := exec.CommandContext(ctx, "xdotool", append([]string{"key", "--clearmodifiers"}, keys...)...)
	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return fmt.Errorf("xdotool backspace failed: %w", err)
	}
	return nil
}
