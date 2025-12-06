package notifier

import (
	"fmt"
	"log/slog"
	"slices"
	"sync"
	"time"

	"github.com/godbus/dbus/v5"
	"github.com/kabilan108/dictator/internal/ipc"
)

type NotificationContent struct {
	Title string
	Body  string
	Icon  string
}

type Notifier interface {
	UpdateState(state ipc.DaemonState) error
	UpdateStateWithDuration(state ipc.DaemonState, duration time.Duration) error
	Update(title, body string) error
	Close() error
}

type DBusNotifier struct {
	conn           *dbus.Conn
	notificationID uint32
	mu             sync.Mutex
}

var stateNotifications = map[ipc.DaemonState]NotificationContent{
	ipc.StateIdle: {
		Title: "dictator",
		Body:  "ready for voice input",
		Icon:  "audio-input-microphone",
	},
	ipc.StateRecording: {
		Title: "dictator",
		Body:  "Recording audio",
		Icon:  "media-record",
	},
	ipc.StateTranscribing: {
		Title: "dictator",
		Body:  "transcribing audio",
		Icon:  "process-working-symbolic",
	},
	ipc.StateTyping: {
		Title: "dictator",
		Body:  "typing text",
		Icon:  "input-keyboard",
	},
	ipc.StateError: {
		Title: "dictator",
		Body:  "an error occurred",
		Icon:  "dialog-error",
	},
}

const (
	dbusService   = "org.freedesktop.Notifications"
	dbusPath      = "/org/freedesktop/Notifications"
	dbusInterface = "org.freedesktop.Notifications"
	methodNotify  = "org.freedesktop.Notifications.Notify"
	methodClose   = "org.freedesktop.Notifications.CloseNotification"
)

// formatDuration formats a duration into a readable string (e.g., "0:15", "1:30")
func formatDuration(d time.Duration) string {
	totalSeconds := int(d.Seconds())
	minutes := totalSeconds / 60
	seconds := totalSeconds % 60
	return fmt.Sprintf("%d:%02d", minutes, seconds)
}

func New() (Notifier, error) {
	conn, err := dbus.ConnectSessionBus()
	if err != nil {
		slog.Error("failed to connect to session D-Bus", "err", err)
		return nil, fmt.Errorf("failed to connect to D-Bus session bus: %w", err)
	}

	// test connection by checking if notifications service is available
	var names []string
	err = conn.BusObject().Call("org.freedesktop.DBus.ListNames", 0).Store(&names)
	if err != nil {
		conn.Close()
		slog.Error("failed to list D-Bus names", "err", err)
		return nil, fmt.Errorf("failed to query D-Bus services: %w", err)
	}

	serviceAvailable := slices.Contains(names, dbusService)
	if !serviceAvailable {
		conn.Close()
		slog.Warn("notification service not available, D-Bus notification service may not be running")
		return nil, fmt.Errorf("notification service %s not available", dbusService)
	}

	notifier := &DBusNotifier{
		conn:           conn,
		notificationID: 0, // 0 means create new notification
	}

	slog.Debug("dbus notifier initialized successfully")
	return notifier, nil
}

// updatestate updates the notification based on the current daemon state
func (n *DBusNotifier) UpdateState(state ipc.DaemonState) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	content, exists := stateNotifications[state]
	if !exists {
		slog.Error("unknown notification state", "state", state)
		return fmt.Errorf("unknown notification state: %d", state)
	}

	slog.Debug("updating notification state", "title", content.Title, "body", content.Body)
	return n.updateNotification(content.Title, content.Body, content.Icon)
}

// UpdateStateWithDuration updates the notification with current recording duration
func (n *DBusNotifier) UpdateStateWithDuration(state ipc.DaemonState, duration time.Duration) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	content, exists := stateNotifications[state]
	if !exists {
		slog.Error("unknown notification state", "state", state)
		return fmt.Errorf("unknown notification state: %d", state)
	}

	// Format duration and update body for recording state
	if state == ipc.StateRecording {
		formattedDuration := formatDuration(duration)
		updatedBody := fmt.Sprintf("Recording audio %s", formattedDuration)
		slog.Debug("updating recording notification with duration", "duration", formattedDuration)
		return n.updateNotification(content.Title, updatedBody, content.Icon)
	}

	// For non-recording states, use standard notification
	slog.Debug("updating notification state", "title", content.Title, "body", content.Body)
	return n.updateNotification(content.Title, content.Body, content.Icon)
}

// update sends a custom notification with specified title, and body
func (n *DBusNotifier) Update(title, body string) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	slog.Debug("sending custom notification", "title", title)
	return n.updateNotification(title, body, "")
}

// Close dismisses the current notification
func (n *DBusNotifier) Close() error {
	n.mu.Lock()
	defer n.mu.Unlock()

	if n.conn == nil {
		slog.Warn("connection already closed")
		return nil
	}

	if n.notificationID != 0 {
		call := n.conn.Object(dbusService, dbusPath).Call(methodClose, 0, n.notificationID)
		if call.Err != nil {
			slog.Warn("failed to close notification", "err", call.Err)
		} else {
			slog.Debug("notification closed", "id", n.notificationID)
		}
		n.notificationID = 0
	}

	if err := n.conn.Close(); err != nil {
		slog.Error("failed to close D-Bus connection", "err", err)
		return fmt.Errorf("failed to close D-Bus connection: %w", err)
	}

	n.conn = nil
	slog.Debug("dbus notifier closed")
	return nil
}

// updateNotification sends notification via D-Bus
func (n *DBusNotifier) updateNotification(title, body, icon string) error {
	if n.conn == nil {
		return fmt.Errorf("D-Bus connection is closed")
	}

	// Notification parameters according to freedesktop.org spec
	appName := "dictator"
	replaceID := n.notificationID
	actions := []string{} // No actions for now
	hints := map[string]dbus.Variant{
		"urgency": dbus.MakeVariant(byte(1)), // Normal urgency
	}
	timeout := int32(-1) // Use default timeout

	// Call the Notify method
	call := n.conn.Object(dbusService, dbusPath).Call(
		methodNotify, 0,
		appName,   // app_name
		replaceID, // replaces_id (0 for new, or existing ID to update)
		icon,      // app_icon
		title,     // summary
		body,      // body
		actions,   // actions
		hints,     // hints
		timeout,   // timeout
	)

	if call.Err != nil {
		slog.Error("failed to send notification", "err", call.Err)
		return fmt.Errorf("failed to send notification: %w", call.Err)
	}

	// Get the notification ID from the response
	var newID uint32
	if err := call.Store(&newID); err != nil {
		slog.Error("failed to get notification ID", "err", err)
		return fmt.Errorf("failed to get notification ID: %w", err)
	}

	n.notificationID = newID
	slog.Debug("notification sent successfully", "id", newID)
	return nil
}
