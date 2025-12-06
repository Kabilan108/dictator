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
		typer := &WaylandTyper{}
		if typer.IsAvailable() {
			slog.Debug("using wtype for text input (wayland)")
			return typer, nil
		}
		return nil, fmt.Errorf("wayland detected but wtype not available")
	}

	typer := &X11Typer{}
	if typer.IsAvailable() {
		slog.Debug("using xclip/xdotool for text input (x11)")
		return typer, nil
	}
	return nil, fmt.Errorf("x11 detected but xclip/xdotool not available")
}

// checks if the required commands are installed
func areInstalled(cmds ...string) bool {
	for _, cmd := range cmds {
		if _, err := exec.LookPath(cmd); err != nil {
			return false
		}
	}
	return true
}

// returns a closure that can be used to type text
func typeFunc(
	ctx context.Context, copyCmd []string, pasteCmd []string,
) func(text string) error {
	return func(text string) error {
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
}

// uses xclip to copy to clipboard and xdotool to paste
type X11Typer struct{}

func (x *X11Typer) IsAvailable() bool { return areInstalled("xclip", "xdotool") }
func (x *X11Typer) Type(ctx context.Context, text string) error {
	copyCmd := []string{"xclip", "-selection", "clipboard"}
	pasteCmd := []string{"xdotool", "key", "ctrl+shift+v"}
	return typeFunc(ctx, copyCmd, pasteCmd)(text)
}

// uses wl-copy to copy to clipboard and wtype to paste
type WaylandTyper struct{}

func (w *WaylandTyper) IsAvailable() bool { return areInstalled("wl-copy", "wtype") }
func (w *WaylandTyper) Type(ctx context.Context, text string) error {
	copyCmd := []string{"wl-copy"}
	pasteCmd := []string{
		"wtype", "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "ctrl", "-m", "shift",
	}
	return typeFunc(ctx, copyCmd, pasteCmd)(text)
}
