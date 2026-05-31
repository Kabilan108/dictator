package visual

import "time"

type EventType string

const (
	EventTypeState EventType = "state"
	EventTypeMeter EventType = "meter"
)

type StateValue string

const (
	StateIdle         StateValue = "idle"
	StateRecording    StateValue = "recording"
	StateTranscribing StateValue = "transcribing"
	StateTyping       StateValue = "typing"
	StateError        StateValue = "error"
)

type Event interface {
	visualEvent()
}

type StateEvent struct {
	Type                EventType  `json:"type"`
	Value               StateValue `json:"value"`
	RecordingDurationMS *int64     `json:"recording_duration_ms,omitempty"`
	Message             string     `json:"message,omitempty"`
}

type MeterEvent struct {
	Type EventType `json:"type"`
	RMS  float64   `json:"rms"`
	Peak float64   `json:"peak"`
}

func NewStateEvent(value StateValue, recordingDuration *time.Duration, message string) StateEvent {
	event := StateEvent{
		Type:    EventTypeState,
		Value:   value,
		Message: message,
	}

	if recordingDuration != nil {
		durationMS := recordingDuration.Milliseconds()
		event.RecordingDurationMS = &durationMS
	}

	return event
}

func NewMeterEvent(rms, peak float64) MeterEvent {
	return MeterEvent{
		Type: EventTypeMeter,
		RMS:  rms,
		Peak: peak,
	}
}

func (StateEvent) visualEvent() {}

func (MeterEvent) visualEvent() {}
