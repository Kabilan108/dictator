package visual

import (
	"bufio"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestSocketSinkSendsSnapshotAndEvents(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())

	duration := 1234 * time.Millisecond
	sink, err := NewSocketSink(func() StateEvent {
		return NewStateEvent(StateRecording, &duration, "")
	})
	if err != nil {
		t.Fatalf("NewSocketSink() error = %v", err)
	}
	t.Cleanup(func() {
		if err := sink.Close(); err != nil {
			t.Fatalf("Close() error = %v", err)
		}
	})

	conn, err := net.Dial("unix", DefaultSocketPath())
	if err != nil {
		t.Fatalf("Dial() error = %v", err)
	}
	t.Cleanup(func() {
		if err := conn.Close(); err != nil {
			t.Fatalf("conn.Close() error = %v", err)
		}
	})

	if err := conn.SetReadDeadline(time.Now().Add(time.Second)); err != nil {
		t.Fatalf("SetReadDeadline() error = %v", err)
	}

	reader := bufio.NewReader(conn)
	line, err := reader.ReadBytes('\n')
	if err != nil {
		t.Fatalf("ReadBytes(snapshot) error = %v", err)
	}

	var snapshot StateEvent
	if err := json.Unmarshal(line, &snapshot); err != nil {
		t.Fatalf("snapshot json.Unmarshal() error = %v", err)
	}
	if snapshot.Value != StateRecording {
		t.Fatalf("snapshot.Value = %q, want %q", snapshot.Value, StateRecording)
	}
	if snapshot.RecordingDurationMS == nil || *snapshot.RecordingDurationMS != 1234 {
		t.Fatalf("snapshot.RecordingDurationMS = %v, want 1234", snapshot.RecordingDurationMS)
	}

	sink.Publish(NewMeterEvent(0.25, 0.5))

	line, err = reader.ReadBytes('\n')
	if err != nil {
		t.Fatalf("ReadBytes(meter) error = %v", err)
	}

	var meter MeterEvent
	if err := json.Unmarshal(line, &meter); err != nil {
		t.Fatalf("meter json.Unmarshal() error = %v", err)
	}
	if meter.Type != EventTypeMeter || meter.RMS != 0.25 || meter.Peak != 0.5 {
		t.Fatalf("meter = %#v, want rms=0.25 peak=0.5", meter)
	}
}

func TestDefaultSocketPathFallbackIsUserScoped(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", "")
	t.Setenv("USER", "dictator-test")

	path := DefaultSocketPath()
	if !strings.HasSuffix(path, filepath.Join("dictator-osd-dictator-test", "osd.sock")) {
		t.Fatalf("DefaultSocketPath() = %q, want user-scoped temp socket", path)
	}
}

func TestSocketSinkDoesNotRemoveActiveSocket(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())

	socketPath := DefaultSocketPath()
	if err := os.MkdirAll(filepath.Dir(socketPath), 0o700); err != nil {
		t.Fatalf("MkdirAll() error = %v", err)
	}

	listener, err := net.Listen("unix", socketPath)
	if err != nil {
		t.Fatalf("Listen() error = %v", err)
	}
	t.Cleanup(func() {
		if err := listener.Close(); err != nil {
			t.Fatalf("listener.Close() error = %v", err)
		}
	})

	_, err = NewSocketSink(nil)
	if err == nil {
		t.Fatal("NewSocketSink() succeeded with active socket, want error")
	}

	conn, err := net.Dial("unix", socketPath)
	if err != nil {
		t.Fatalf("active socket was removed or broken: %v", err)
	}
	if err := conn.Close(); err != nil {
		t.Fatalf("conn.Close() error = %v", err)
	}
}

func TestSocketSinkRejectsClientsOverLimit(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())

	sink, err := NewSocketSink(nil)
	if err != nil {
		t.Fatalf("NewSocketSink() error = %v", err)
	}
	t.Cleanup(func() {
		if err := sink.Close(); err != nil {
			t.Fatalf("Close() error = %v", err)
		}
	})

	var conns []net.Conn
	t.Cleanup(func() {
		for _, conn := range conns {
			if err := conn.Close(); err != nil {
				t.Fatalf("conn.Close() error = %v", err)
			}
		}
	})

	for range maxClients {
		conn, err := net.Dial("unix", DefaultSocketPath())
		if err != nil {
			t.Fatalf("Dial() error = %v", err)
		}
		if err := conn.SetReadDeadline(time.Now().Add(time.Second)); err != nil {
			t.Fatalf("SetReadDeadline() error = %v", err)
		}
		if _, err := bufio.NewReader(conn).ReadBytes('\n'); err != nil {
			t.Fatalf("ReadBytes(snapshot) error = %v", err)
		}
		conns = append(conns, conn)
	}

	extra, err := net.Dial("unix", DefaultSocketPath())
	if err != nil {
		t.Fatalf("extra Dial() error = %v", err)
	}
	defer extra.Close()

	if err := extra.SetReadDeadline(time.Now().Add(250 * time.Millisecond)); err != nil {
		t.Fatalf("SetReadDeadline() error = %v", err)
	}
	if _, err := bufio.NewReader(extra).ReadBytes('\n'); err == nil {
		t.Fatal("extra client received a snapshot, want rejected connection")
	}
}
