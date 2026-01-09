package ipc

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net"
	"os"
	"sync"
	"time"
)

const ServerConnectionDeadline = 30 * time.Second

// CommandHandler defines the interface for handling daemon commands
type CommandHandler interface {
	HandleStart() error
	HandleStop() error
	HandleToggle() error
	HandleCancel() error
	GetStatus() StatusData
}

// Server represents the IPC server that listens for CLI commands
type Server struct {
	socketPath string
	listener   net.Listener
	handler    CommandHandler

	mu      sync.RWMutex
	running bool

	ctx    context.Context
	cancel context.CancelFunc
}

func NewServer(handler CommandHandler) *Server {
	ctx, cancel := context.WithCancel(context.Background())

	return &Server{
		socketPath: SocketPath,
		handler:    handler,
		running:    false,
		ctx:        ctx,
		cancel:     cancel,
	}
}

func (s *Server) Start() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	slog.Debug("starting ipc server", "path", s.socketPath)

	if s.running {
		return fmt.Errorf("server is already running")
	}

	if err := os.Remove(s.socketPath); err != nil && !os.IsNotExist(err) {
		slog.Warn("failed to remove existing socket file", "err", err)
	}

	// create unix socket listener
	listener, err := net.Listen("unix", s.socketPath)
	if err != nil {
		return err
	}

	s.listener = listener
	s.running = true

	go s.acceptConnections()
	return nil
}

func (s *Server) Stop() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if !s.running {
		return nil
	}

	slog.Debug("stopping ipc server")

	// cancel context to stop all operations
	s.cancel()

	if s.listener != nil {
		if err := s.listener.Close(); err != nil {
			slog.Error("failed to close listener", "err", err)
		}
	}

	if err := os.Remove(s.socketPath); err != nil && !os.IsNotExist(err) {
		slog.Warn("failed to remove socket file", "err", err)
	}

	s.running = false
	return nil
}

func (s *Server) acceptConnections() {
	for {
		select {
		case <-s.ctx.Done():
			slog.Debug("accept loop terminated due to context cancellation")
			return
		default:
		}

		conn, err := s.listener.Accept()
		if err != nil {
			select {
			case <-s.ctx.Done():
				// Server is shutting down, this is expected
				return
			default:
				slog.Warn("failed to accept connection", "err", err)
				continue
			}
		}

		slog.Debug("new client connection accepted")

		// Handle connection in goroutine
		go s.handleConnection(conn)
	}
}

func (s *Server) handleConnection(conn net.Conn) {
	defer func() {
		if err := conn.Close(); err != nil {
			slog.Warn("failed to close connection", "err", err)
		}
		slog.Debug("client connection closed")
	}()

	// set connection timeout
	if err := conn.SetDeadline(time.Now().Add(ServerConnectionDeadline)); err != nil {
		slog.Warn("failed to set connection deadline", "err", err)
	}

	// decode command from connection
	var cmd Command
	decoder := json.NewDecoder(conn)
	if err := decoder.Decode(&cmd); err != nil {
		slog.Error("failed to decode command", "err", err)
		s.sendErrorResponse(conn, "", ErrInvalidCommand, err)
		return
	}

	slog.Debug("received command", "action", cmd.Action, "id", cmd.ID)

	// process command and send response
	response := s.processCommand(&cmd)
	s.sendResponse(conn, response)
}

func (s *Server) processCommand(cmd *Command) *Response {
	response := &Response{
		ID:      cmd.ID,
		Success: false,
		Data:    make(map[string]string),
	}

	var err error

	switch cmd.Action {
	case ActionStart:
		err = s.handler.HandleStart()
		if err == nil {
			response.Success = true
			response.Data[DataKeyState] = StateRecording.String()
		}

	case ActionStop:
		err = s.handler.HandleStop()
		if err == nil {
			response.Success = true
			response.Data[DataKeyState] = StateIdle.String()
		}

	case ActionToggle:
		err = s.handler.HandleToggle()
		if err == nil {
			response.Success = true
			// State will be determined by the handler
		}

	case ActionCancel:
		err = s.handler.HandleCancel()
		if err == nil {
			response.Success = true
			response.Data[DataKeyState] = StateIdle.String()
		}

	case ActionStatus:
		status := s.handler.GetStatus()
		response.Success = true
		response.Data[DataKeyState] = status.State.String()
		response.Data[DataKeyUptime] = status.Uptime.String()

		if status.RecordingDuration != nil {
			response.Data[DataKeyRecordingDuration] = status.RecordingDuration.String()
		}
		if status.LastError != nil {
			response.Data[DataKeyLastError] = *status.LastError
		}

	default:
		err = fmt.Errorf("unknown action: %s", cmd.Action)
		response.Error = ErrInvalidCommand
	}

	if err != nil && response.Error == "" {
		response.Error = err.Error()
		slog.Error("command failed", "err", err)
	}

	return response
}

func (s *Server) sendResponse(conn net.Conn, response *Response) {
	encoder := json.NewEncoder(conn)
	if err := encoder.Encode(response); err != nil {
		slog.Error("failed to encode response", "err", err)
		return
	}

	if response.Success {
		slog.Debug("sent success response", "id", response.ID)
	} else {
		slog.Debug("sent error response", "id", response.ID, "error", response.Error)
	}
}

func (s *Server) sendErrorResponse(conn net.Conn, id, errorMsg string, originalErr error) {
	response := &Response{
		ID:      id,
		Success: false,
		Error:   errorMsg,
	}

	if originalErr != nil {
		slog.Error("sending error response", "err", originalErr)
	}

	s.sendResponse(conn, response)
}
