package overlay

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"sync"
	"time"
)

const socketPath = "/tmp/dictator-overlay.sock"

type Message struct {
	Type      string `json:"type"`
	Text      string `json:"text,omitempty"`
	StableLen int    `json:"stable_len,omitempty"`
}

type Manager struct {
	cmd    *exec.Cmd
	conn   net.Conn
	connMu sync.Mutex

	onConfirm func()
	onCancel  func()
}

func NewManager() *Manager {
	return &Manager{}
}

func (m *Manager) SetHandlers(onConfirm, onCancel func()) {
	m.onConfirm = onConfirm
	m.onCancel = onCancel
}

func (m *Manager) Start() error {
	overlayPath, err := exec.LookPath("dictator-overlay")
	if err != nil {
		return fmt.Errorf("overlay not found: %w", err)
	}

	m.cmd = exec.Command(overlayPath)
	m.cmd.Stdout = os.Stdout
	m.cmd.Stderr = os.Stderr

	if err := m.cmd.Start(); err != nil {
		return fmt.Errorf("failed to start overlay: %w", err)
	}

	for i := 0; i < 50; i++ {
		time.Sleep(100 * time.Millisecond)
		if _, err := os.Stat(socketPath); err == nil {
			break
		}
	}

	conn, err := net.Dial("unix", socketPath)
	if err != nil {
		m.cmd.Process.Kill()
		return fmt.Errorf("failed to connect to overlay: %w", err)
	}

	m.connMu.Lock()
	m.conn = conn
	m.connMu.Unlock()

	go m.receiveLoop()

	return nil
}

func (m *Manager) Update(text string, stableLen int) error {
	msg := Message{
		Type:      "update",
		Text:      text,
		StableLen: stableLen,
	}
	return m.send(msg)
}

func (m *Manager) Show() error {
	return m.send(Message{Type: "show"})
}

func (m *Manager) Hide() error {
	return m.send(Message{Type: "hide"})
}

func (m *Manager) Stop() error {
	m.connMu.Lock()
	if m.conn != nil {
		m.conn.Close()
		m.conn = nil
	}
	m.connMu.Unlock()

	if m.cmd != nil && m.cmd.Process != nil {
		m.cmd.Process.Kill()
		m.cmd.Wait()
	}

	return nil
}

func (m *Manager) send(msg Message) error {
	m.connMu.Lock()
	defer m.connMu.Unlock()

	if m.conn == nil {
		return fmt.Errorf("not connected")
	}

	data, err := json.Marshal(msg)
	if err != nil {
		return err
	}

	_, err = m.conn.Write(data)
	return err
}

func (m *Manager) receiveLoop() {
	buf := make([]byte, 4096)

	for {
		m.connMu.Lock()
		conn := m.conn
		m.connMu.Unlock()

		if conn == nil {
			return
		}

		n, err := conn.Read(buf)
		if err != nil {
			return
		}

		var msg Message
		if err := json.Unmarshal(buf[:n], &msg); err != nil {
			continue
		}

		switch msg.Type {
		case "confirm":
			if m.onConfirm != nil {
				m.onConfirm()
			}
		case "cancel":
			if m.onCancel != nil {
				m.onCancel()
			}
		}
	}
}
