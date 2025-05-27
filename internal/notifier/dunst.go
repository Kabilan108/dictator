package notifier

import (
	"fmt"
	"slices"
	"sync"

	"github.com/godbus/dbus/v5"
	"github.com/kabilan108/dictator/internal/utils"
)

type NotificationState int

const (
	StateIdle NotificationState = iota
	StateRecording
	StateTranscribing
	StateTyping
	StateError
)

type NotificationContent struct {
	Title string
	Body  string
	Icon  string
}

type Notifier interface {
	UpdateState(state NotificationState) error
	Update(title, body, icon string) error
	Close() error
}

type DunstNotifier struct {
	conn           *dbus.Conn
	notificationID uint32
	log            utils.Logger
	mu             sync.Mutex
}

var stateNotifications = map[NotificationState]NotificationContent{
	StateIdle: {
		Title: "🏠 Dictator",
		Body:  "Ready for voice input",
		Icon:  "microphone-sensitivity-muted",
	},
	StateRecording: {
		Title: "🎤 Dictator",
		Body:  "Recording audio...",
		Icon:  "audio-input-microphone",
	},
	StateTranscribing: {
		Title: "🔄 Dictator",
		Body:  "Transcribing audio...",
		Icon:  "system-run",
	},
	StateTyping: {
		Title: "⌨️ Dictator",
		Body:  "Typing text...",
		Icon:  "input-keyboard",
	},
	StateError: {
		Title: "❌ Dictator",
		Body:  "An error occurred",
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

func New(logLevel utils.LogLevel) (Notifier, error) {
	log := utils.NewLogger(logLevel, "notifier")

	conn, err := dbus.ConnectSessionBus()
	if err != nil {
		log.E("failed to connect to session D-Bus: %v", err)
		return nil, fmt.Errorf("failed to connect to D-Bus session bus: %w", err)
	}

	// test connection by checking if notifications service is available
	var names []string
	err = conn.BusObject().Call("org.freedesktop.DBus.ListNames", 0).Store(&names)
	if err != nil {
		conn.Close()
		log.E("failed to list D-Bus names: %v", err)
		return nil, fmt.Errorf("failed to query D-Bus services: %w", err)
	}

	serviceAvailable := slices.Contains(names, dbusService)
	if !serviceAvailable {
		conn.Close()
		log.W("notification service not available, dunst may not be running")
		return nil, fmt.Errorf("notification service %s not available", dbusService)
	}

	notifier := &DunstNotifier{
		conn:           conn,
		notificationID: 0, // 0 means create new notification
		log:            log,
	}

	log.D("dunst notifier initialized successfully")
	return notifier, nil
}

// updatestate updates the notification based on the current daemon state
func (n *DunstNotifier) UpdateState(state NotificationState) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	content, exists := stateNotifications[state]
	if !exists {
		n.log.E("unknown notification state: %d", state)
		return fmt.Errorf("unknown notification state: %d", state)
	}

	n.log.D("updating notification state to: %s", content.Title)
	return n.updateNotification(content.Title, content.Body, content.Icon)
}

// update sends a custom notification with specified title, body, and icon
func (n *DunstNotifier) Update(title, body, icon string) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	n.log.D("sending custom notification: %s", title)
	return n.updateNotification(title, body, icon)
}

// Close dismisses the current notification
func (n *DunstNotifier) Close() error {
	n.mu.Lock()
	defer n.mu.Unlock()

	if n.conn == nil {
		n.log.W("connection already closed")
		return nil
	}

	if n.notificationID != 0 {
		call := n.conn.Object(dbusService, dbusPath).Call(methodClose, 0, n.notificationID)
		if call.Err != nil {
			n.log.W("failed to close notification: %v", call.Err)
		} else {
			n.log.D("notification %d closed", n.notificationID)
		}
		n.notificationID = 0
	}

	if err := n.conn.Close(); err != nil {
		n.log.E("failed to close D-Bus connection: %v", err)
		return fmt.Errorf("failed to close D-Bus connection: %w", err)
	}

	n.conn = nil
	n.log.D("dunst notifier closed")
	return nil
}

// updateNotification sends notification via D-Bus
func (n *DunstNotifier) updateNotification(title, body, icon string) error {
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
		n.log.E("failed to send notification: %v", call.Err)
		return fmt.Errorf("failed to send notification: %w", call.Err)
	}

	// Get the notification ID from the response
	var newID uint32
	if err := call.Store(&newID); err != nil {
		n.log.E("failed to get notification ID: %v", err)
		return fmt.Errorf("failed to get notification ID: %w", err)
	}

	n.notificationID = newID
	n.log.D("notification sent successfully (ID: %d)", newID)
	return nil
}
