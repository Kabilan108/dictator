package streaming

import (
	"context"
	"fmt"
	"sync"

	"github.com/kabilan108/dictator/internal/overlay"
	"github.com/kabilan108/dictator/internal/typing"
)

type Handler struct {
	client *Client
	typer  typing.StreamingTyper

	mu       sync.Mutex
	typedLen int
	lastText string

	overlayMode bool
	overlay     *overlay.Manager

	onStateChange func(state string)
}

func NewHandler(client *Client, typer typing.StreamingTyper, overlayMode bool) *Handler {
	h := &Handler{
		client:      client,
		typer:       typer,
		overlayMode: overlayMode,
	}

	if overlayMode {
		h.overlay = overlay.NewManager()
	}

	return h
}

func (h *Handler) SetStateCallback(cb func(state string)) {
	h.onStateChange = cb
}

func (h *Handler) Start(ctx context.Context) error {
	if h.overlayMode && h.overlay != nil {
		if err := h.overlay.Start(); err != nil {
			return fmt.Errorf("failed to start overlay: %w", err)
		}

		h.overlay.SetHandlers(
			func() {
				h.mu.Lock()
				text := h.lastText
				h.mu.Unlock()
				h.typer.Type(context.Background(), text)
			},
			func() {
				// Cancel - don't type anything
			},
		)
	}

	h.client.SetHandlers(
		h.handlePartial,
		h.handleFinal,
		h.handleError,
	)

	if err := h.client.Connect(ctx); err != nil {
		if h.overlay != nil {
			h.overlay.Stop()
		}
		return err
	}

	if h.onStateChange != nil {
		h.onStateChange("streaming")
	}

	return nil
}

func (h *Handler) SendAudio(pcmData []byte) error {
	return h.client.SendAudio(pcmData)
}

func (h *Handler) Stop(ctx context.Context) (string, error) {
	if err := h.client.End(); err != nil {
		return "", err
	}

	h.client.Wait()

	h.mu.Lock()
	finalText := h.lastText
	h.mu.Unlock()

	if h.overlay != nil {
		h.overlay.Stop()
	}

	if h.onStateChange != nil {
		h.onStateChange("idle")
	}

	return finalText, nil
}

func (h *Handler) Cancel() {
	h.client.Close()
	if h.overlay != nil {
		h.overlay.Stop()
	}
	if h.onStateChange != nil {
		h.onStateChange("idle")
	}
}

func (h *Handler) handlePartial(text string, stableLen int, seq int) {
	h.mu.Lock()
	defer h.mu.Unlock()

	h.lastText = text

	if h.overlayMode && h.overlay != nil {
		h.overlay.Update(text, stableLen)
	} else {
		if stableLen > h.typedLen {
			newText := text[h.typedLen:stableLen]
			h.typer.TypeIncremental(context.Background(), newText)
			h.typedLen = stableLen
		}
	}
}

func (h *Handler) handleFinal(text string) {
	h.mu.Lock()
	defer h.mu.Unlock()

	h.lastText = text

	if h.overlayMode && h.overlay != nil {
		h.overlay.Update(text, len(text))
	} else {
		if len(text) > h.typedLen {
			remaining := text[h.typedLen:]
			h.typer.TypeIncremental(context.Background(), remaining)
			h.typedLen = len(text)
		}
	}
}

func (h *Handler) handleError(err error) {
	if h.overlay != nil {
		h.overlay.Stop()
	}
	if h.onStateChange != nil {
		h.onStateChange("error")
	}
}
