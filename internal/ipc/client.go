package ipc

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net"
	"time"

	"github.com/google/uuid"
)

// client represents an ipc client for communicating with the daemon
type Client struct {
	socketPath string
	timeout    time.Duration
}

func NewClient() *Client {
	return &Client{
		socketPath: SocketPath,
		timeout:    10 * time.Second,
	}
}

func (c *Client) SendCommand(ctx context.Context, action string, args ...string) (*Response, error) {
	cmd := Command{
		ID:        uuid.New().String(),
		Action:    action,
		Args:      args,
		Timestamp: time.Now(),
	}

	slog.Debug("sending command", "action", cmd.Action, "id", cmd.ID)

	timeoutCtx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()

	// Connect to daemon
	conn, err := c.connect(timeoutCtx)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to daemon: %w", err)
	}
	defer func() {
		if closeErr := conn.Close(); closeErr != nil {
			slog.Warn("failed to close connection", "err", closeErr)
		}
	}()

	// Set connection deadline
	deadline, ok := timeoutCtx.Deadline()
	if ok {
		if err := conn.SetDeadline(deadline); err != nil {
			slog.Warn("failed to set connection deadline", "err", err)
		}
	}

	// Send command
	encoder := json.NewEncoder(conn)
	if err := encoder.Encode(&cmd); err != nil {
		slog.Error("failed to encode command", "err", err)
		return nil, fmt.Errorf("failed to send command: %w", err)
	}

	// Receive response
	var response Response
	decoder := json.NewDecoder(conn)
	if err := decoder.Decode(&response); err != nil {
		slog.Error("failed to decode response", "err", err)
		return nil, fmt.Errorf("failed to receive response: %w", err)
	}

	// Validate response ID matches command ID
	if response.ID != cmd.ID {
		slog.Error("response ID mismatch", "expected", cmd.ID, "got", response.ID)
		return nil, fmt.Errorf("response ID mismatch")
	}

	slog.Debug("received response", "action", cmd.Action, "success", response.Success)
	return &response, nil
}

func (c *Client) connect(ctx context.Context) (net.Conn, error) {
	// Use net.Dialer with context for timeout support
	dialer := &net.Dialer{}
	conn, err := dialer.DialContext(ctx, "unix", c.socketPath)
	if err != nil {
		slog.Error("failed to dial unix socket", "err", err)
		return nil, err
	}

	slog.Debug("connected to daemon", "path", c.socketPath)
	return conn, nil
}

func (c *Client) Start(ctx context.Context) (*Response, error) {
	return c.SendCommand(ctx, ActionStart)
}

func (c *Client) Stop(ctx context.Context) (*Response, error) {
	return c.SendCommand(ctx, ActionStop)
}

func (c *Client) Toggle(ctx context.Context) (*Response, error) {
	return c.SendCommand(ctx, ActionToggle)
}

func (c *Client) Cancel(ctx context.Context) (*Response, error) {
	return c.SendCommand(ctx, ActionCancel)
}

func (c *Client) Status(ctx context.Context) (*Response, error) {
	return c.SendCommand(ctx, ActionStatus)
}

func (c *Client) IsConnected(ctx context.Context) bool {
	// Create a short timeout context for the connection test
	testCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()

	conn, err := c.connect(testCtx)
	if err != nil {
		return false
	}
	defer func() {
		if closeErr := conn.Close(); closeErr != nil {
			slog.Warn("failed to close test connection", "err", closeErr)
		}
	}()

	return true
}

// WaitForDaemon waits for the daemon to become available
func (c *Client) WaitForDaemon(ctx context.Context, checkInterval time.Duration) error {
	slog.Debug("waiting for daemon to become available")

	ticker := time.NewTicker(checkInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
			if c.IsConnected(ctx) {
				slog.Debug("daemon is now available")
				return nil
			}
		}
	}
}
