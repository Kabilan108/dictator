package ipc

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"time"

	"github.com/google/uuid"
	"github.com/kabilan108/dictator/internal/utils"
)

// client represents an ipc client for communicating with the daemon
type Client struct {
	socketPath string
	timeout    time.Duration
	log        utils.Logger
}

func NewClient(logLevel utils.LogLevel) *Client {
	return &Client{
		socketPath: SocketPath,
		timeout:    10 * time.Second,
		log:        utils.NewLogger(logLevel, "ipc-client"),
	}
}

func (c *Client) SendCommand(ctx context.Context, action string, args ...string) (*Response, error) {
	cmd := Command{
		ID:        uuid.New().String(),
		Action:    action,
		Args:      args,
		Timestamp: time.Now(),
	}

	c.log.D("sending command: %s (ID: %s)", cmd.Action, cmd.ID)

	timeoutCtx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()

	// Connect to daemon
	conn, err := c.connect(timeoutCtx)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to daemon: %w", err)
	}
	defer func() {
		if closeErr := conn.Close(); closeErr != nil {
			c.log.W("failed to close connection: %v", closeErr)
		}
	}()

	// Set connection deadline
	deadline, ok := timeoutCtx.Deadline()
	if ok {
		if err := conn.SetDeadline(deadline); err != nil {
			c.log.W("failed to set connection deadline: %v", err)
		}
	}

	// Send command
	encoder := json.NewEncoder(conn)
	if err := encoder.Encode(&cmd); err != nil {
		c.log.E("failed to encode command: %v", err)
		return nil, fmt.Errorf("failed to send command: %w", err)
	}

	// Receive response
	var response Response
	decoder := json.NewDecoder(conn)
	if err := decoder.Decode(&response); err != nil {
		c.log.E("failed to decode response: %v", err)
		return nil, fmt.Errorf("failed to receive response: %w", err)
	}

	// Validate response ID matches command ID
	if response.ID != cmd.ID {
		c.log.E("response ID mismatch: expected %s, got %s", cmd.ID, response.ID)
		return nil, fmt.Errorf("response ID mismatch")
	}

	c.log.D("received response for command %s: success=%v", cmd.Action, response.Success)

	return &response, nil
}

func (c *Client) connect(ctx context.Context) (net.Conn, error) {
	// Use net.Dialer with context for timeout support
	dialer := &net.Dialer{}
	conn, err := dialer.DialContext(ctx, "unix", c.socketPath)
	if err != nil {
		c.log.E("failed to dial unix socket: %v", err)
		return nil, err
	}

	c.log.D("connected to daemon at %s", c.socketPath)
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
			c.log.W("failed to close test connection: %v", closeErr)
		}
	}()

	return true
}

// WaitForDaemon waits for the daemon to become available
func (c *Client) WaitForDaemon(ctx context.Context, checkInterval time.Duration) error {
	c.log.D("waiting for daemon to become available...")

	ticker := time.NewTicker(checkInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
			if c.IsConnected(ctx) {
				c.log.D("daemon is now available")
				return nil
			}
		}
	}
}
