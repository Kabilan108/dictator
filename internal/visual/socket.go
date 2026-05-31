package visual

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

const (
	reliableQueueSize = 16
	maxClients        = 4
	writeTimeout      = 500 * time.Millisecond
	socketDirPerm     = 0o700
	socketFilePerm    = 0o600
	socketDialTimeout = 100 * time.Millisecond
)

type Sink interface {
	Publish(event Event)
	Close() error
}

type SnapshotFunc func() StateEvent

type NoopSink struct{}

func (NoopSink) Publish(Event) {}

func (NoopSink) Close() error {
	return nil
}

type SocketSink struct {
	socketPath string
	listener   net.Listener
	snapshot   SnapshotFunc

	ctx    context.Context
	cancel context.CancelFunc

	mu      sync.Mutex
	clients map[*client]struct{}
	wg      sync.WaitGroup
}

func DefaultSocketPath() string {
	runtimeDir := os.Getenv("XDG_RUNTIME_DIR")
	if runtimeDir == "" {
		return filepath.Join(os.TempDir(), "dictator-osd-"+socketUserID(), "osd.sock")
	}
	return filepath.Join(runtimeDir, "dictator", "osd.sock")
}

func socketUserID() string {
	if user := os.Getenv("USER"); user != "" {
		return sanitizePathPart(user)
	}
	return fmt.Sprintf("%d", os.Getuid())
}

func sanitizePathPart(value string) string {
	var builder strings.Builder
	builder.Grow(len(value))
	for _, char := range value {
		if char >= 'a' && char <= 'z' || char >= 'A' && char <= 'Z' || char >= '0' && char <= '9' || char == '_' || char == '-' {
			builder.WriteRune(char)
		}
	}
	if builder.Len() == 0 {
		return fmt.Sprintf("%d", os.Getuid())
	}
	return builder.String()
}

func NewSocketSink(snapshot SnapshotFunc) (*SocketSink, error) {
	socketPath := DefaultSocketPath()

	if err := ensureSocketDir(filepath.Dir(socketPath)); err != nil {
		return nil, err
	}

	if err := prepareSocketPath(socketPath); err != nil {
		return nil, err
	}

	listener, err := net.Listen("unix", socketPath)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on OSD socket: %w", err)
	}

	if err := os.Chmod(socketPath, socketFilePerm); err != nil {
		if closeErr := listener.Close(); closeErr != nil {
			slog.Warn("failed to close OSD listener after chmod failure", "err", closeErr)
		}
		return nil, fmt.Errorf("failed to set OSD socket permissions: %w", err)
	}

	if snapshot == nil {
		snapshot = func() StateEvent {
			return NewStateEvent(StateIdle, nil, "")
		}
	}

	ctx, cancel := context.WithCancel(context.Background())
	sink := &SocketSink{
		socketPath: socketPath,
		listener:   listener,
		snapshot:   snapshot,
		ctx:        ctx,
		cancel:     cancel,
		clients:    make(map[*client]struct{}),
	}

	sink.wg.Add(1)
	go sink.acceptConnections()

	slog.Info("OSD event socket started", "path", socketPath)
	return sink, nil
}

func ensureSocketDir(dir string) error {
	if err := os.MkdirAll(dir, socketDirPerm); err != nil {
		return fmt.Errorf("failed to create OSD socket directory: %w", err)
	}

	info, err := os.Stat(dir)
	if err != nil {
		return fmt.Errorf("failed to inspect OSD socket directory: %w", err)
	}
	if !info.IsDir() {
		return fmt.Errorf("OSD socket directory path is not a directory: %s", dir)
	}
	if stat, ok := info.Sys().(*syscall.Stat_t); ok && stat.Uid != uint32(os.Getuid()) {
		return fmt.Errorf("OSD socket directory is owned by uid %d, want %d: %s", stat.Uid, os.Getuid(), dir)
	}
	if info.Mode().Perm() != socketDirPerm {
		if err := os.Chmod(dir, socketDirPerm); err != nil {
			return fmt.Errorf("failed to secure OSD socket directory: %w", err)
		}
	}

	return nil
}

func prepareSocketPath(socketPath string) error {
	info, err := os.Lstat(socketPath)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("failed to inspect OSD socket path: %w", err)
	}
	if info.Mode()&os.ModeSocket == 0 {
		return fmt.Errorf("OSD socket path exists and is not a socket: %s", socketPath)
	}

	conn, err := net.DialTimeout("unix", socketPath, socketDialTimeout)
	if err == nil {
		if closeErr := conn.Close(); closeErr != nil {
			slog.Debug("failed to close active OSD socket probe", "err", closeErr)
		}
		return fmt.Errorf("OSD socket already in use: %s", socketPath)
	}

	if err := os.Remove(socketPath); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("failed to remove stale OSD socket: %w", err)
	}
	return nil
}

func (s *SocketSink) Publish(event Event) {
	s.mu.Lock()
	defer s.mu.Unlock()

	var dropped []*client
	for client := range s.clients {
		switch event := event.(type) {
		case StateEvent:
			if event.Value != StateRecording {
				client.clearMeter()
			}
			if ok := client.publishReliable(event); !ok {
				slog.Debug("dropping slow OSD client", "event", event.Value)
				dropped = append(dropped, client)
			}
		case MeterEvent:
			client.publishMeter(event)
		default:
			slog.Warn("ignoring unknown OSD event", "type", fmt.Sprintf("%T", event))
		}
	}

	for _, client := range dropped {
		s.removeClientLocked(client)
	}
}

func (s *SocketSink) Close() error {
	s.cancel()

	var lastErr error
	if s.listener != nil {
		if err := s.listener.Close(); err != nil {
			lastErr = err
		}
	}

	s.mu.Lock()
	for client := range s.clients {
		client.close()
		delete(s.clients, client)
	}
	s.mu.Unlock()

	s.wg.Wait()

	if err := os.Remove(s.socketPath); err != nil && !os.IsNotExist(err) {
		lastErr = err
	}

	return lastErr
}

func (s *SocketSink) acceptConnections() {
	defer s.wg.Done()

	for {
		conn, err := s.listener.Accept()
		if err != nil {
			select {
			case <-s.ctx.Done():
				return
			default:
				slog.Warn("failed to accept OSD client", "err", err)
				continue
			}
		}

		s.addClient(conn)
	}
}

func (s *SocketSink) addClient(conn net.Conn) {
	client := newClient(conn)

	s.mu.Lock()
	defer s.mu.Unlock()

	if s.ctx.Err() != nil {
		client.close()
		return
	}

	if len(s.clients) >= maxClients {
		slog.Warn("rejecting OSD client because client limit is reached", "limit", maxClients)
		client.close()
		return
	}

	s.clients[client] = struct{}{}
	if ok := client.publishReliable(s.snapshot()); !ok {
		s.removeClientLocked(client)
		return
	}

	s.wg.Add(1)
	go func() {
		defer s.wg.Done()
		client.run()
		s.removeClient(client)
	}()

	slog.Debug("OSD client connected")
}

func (s *SocketSink) removeClient(client *client) {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.removeClientLocked(client)
}

func (s *SocketSink) removeClientLocked(client *client) {
	if _, ok := s.clients[client]; !ok {
		return
	}

	delete(s.clients, client)
	client.close()
	slog.Debug("OSD client disconnected")
}

type client struct {
	conn    net.Conn
	encoder *json.Encoder

	reliable chan Event
	wake     chan struct{}
	done     chan struct{}

	meterMu     sync.Mutex
	latestMeter *MeterEvent

	closeOnce sync.Once
}

func newClient(conn net.Conn) *client {
	return &client{
		conn:     conn,
		encoder:  json.NewEncoder(conn),
		reliable: make(chan Event, reliableQueueSize),
		wake:     make(chan struct{}, 1),
		done:     make(chan struct{}),
	}
}

func (c *client) publishReliable(event Event) bool {
	select {
	case <-c.done:
		return false
	case c.reliable <- event:
		c.wakeWriter()
		return true
	default:
		return false
	}
}

func (c *client) publishMeter(event MeterEvent) {
	c.meterMu.Lock()
	c.latestMeter = &event
	c.meterMu.Unlock()
	c.wakeWriter()
}

func (c *client) clearMeter() {
	c.meterMu.Lock()
	c.latestMeter = nil
	c.meterMu.Unlock()
}

func (c *client) takeMeter() (MeterEvent, bool) {
	c.meterMu.Lock()
	defer c.meterMu.Unlock()

	if c.latestMeter == nil {
		return MeterEvent{}, false
	}

	event := *c.latestMeter
	c.latestMeter = nil
	return event, true
}

func (c *client) wakeWriter() {
	select {
	case c.wake <- struct{}{}:
	default:
	}
}

func (c *client) run() {
	defer c.close()

	for {
		select {
		case <-c.done:
			return
		default:
		}

		select {
		case event := <-c.reliable:
			if ok := c.write(event); !ok {
				return
			}
			continue
		default:
		}

		if event, ok := c.takeMeter(); ok {
			if ok := c.write(event); !ok {
				return
			}
			continue
		}

		select {
		case <-c.done:
			return
		case event := <-c.reliable:
			if ok := c.write(event); !ok {
				return
			}
		case <-c.wake:
		}
	}
}

func (c *client) write(event Event) bool {
	if err := c.conn.SetWriteDeadline(time.Now().Add(writeTimeout)); err != nil {
		slog.Warn("failed to set OSD client write deadline", "err", err)
		return false
	}

	if err := c.encoder.Encode(event); err != nil {
		slog.Warn("failed to write OSD event", "err", err)
		return false
	}

	return true
}

func (c *client) close() {
	c.closeOnce.Do(func() {
		close(c.done)
		if err := c.conn.Close(); err != nil {
			slog.Debug("failed to close OSD client", "err", err)
		}
	})
}
