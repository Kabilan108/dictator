use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateValue {
    Idle,
    Recording,
    Transcribing,
    Typing,
    Error,
}

impl std::fmt::Display for StateValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StateValue::Idle => "idle",
            StateValue::Recording => "recording",
            StateValue::Transcribing => "transcribing",
            StateValue::Typing => "typing",
            StateValue::Error => "error",
        })
    }
}

/// Event published on the OSD socket as newline-delimited JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    State(StateEvent),
    Meter(MeterEvent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateEvent {
    pub value: StateValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// On `recording`: the provider this recording will be transcribed with.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// On `typing`: the text being pasted. The socket is private to the user
    /// (0600 in a 0700 directory), so the transcript may travel on it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl StateEvent {
    pub fn with_provider(mut self, provider: &str) -> Self {
        self.provider = provider.to_string();
        self
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeterEvent {
    pub rms: f64,
    pub peak: f64,
}

pub fn new_state_event(
    value: StateValue,
    recording_duration: Option<Duration>,
    message: &str,
) -> StateEvent {
    StateEvent {
        value,
        recording_duration_ms: recording_duration
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)),
        message: message.to_string(),
        provider: String::new(),
        text: String::new(),
    }
}

pub fn new_meter_event(rms: f64, peak: f64) -> MeterEvent {
    MeterEvent { rms, peak }
}

impl From<StateEvent> for Event {
    fn from(event: StateEvent) -> Self {
        Event::State(event)
    }
}

impl From<MeterEvent> for Event {
    fn from(event: MeterEvent) -> Self {
        Event::Meter(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_like_go() {
        let state: Event =
            new_state_event(StateValue::Recording, Some(Duration::from_millis(1234)), "").into();
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"type":"state","value":"recording","recording_duration_ms":1234}"#
        );

        let err: Event = new_state_event(StateValue::Error, None, "transcription failed").into();
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"type":"state","value":"error","message":"transcription failed"}"#
        );

        let recording: Event = new_state_event(StateValue::Recording, Some(Duration::ZERO), "")
            .with_provider("openai")
            .into();
        assert_eq!(
            serde_json::to_string(&recording).unwrap(),
            r#"{"type":"state","value":"recording","recording_duration_ms":0,"provider":"openai"}"#
        );

        let typing: Event = new_state_event(StateValue::Typing, None, "")
            .with_text("Ship it.")
            .into();
        assert_eq!(
            serde_json::to_string(&typing).unwrap(),
            r#"{"type":"state","value":"typing","text":"Ship it."}"#
        );

        let meter: Event = new_meter_event(0.03, 0.2).into();
        assert_eq!(
            serde_json::to_string(&meter).unwrap(),
            r#"{"type":"meter","rms":0.03,"peak":0.2}"#
        );
    }

    #[test]
    fn provider_and_text_are_optional_when_reading() {
        let event: Event = serde_json::from_str(
            r#"{"type":"state","value":"recording","recording_duration_ms":0}"#,
        )
        .unwrap();
        assert_eq!(
            event,
            new_state_event(StateValue::Recording, Some(Duration::ZERO), "").into()
        );

        let event: Event =
            serde_json::from_str(r#"{"type":"state","value":"typing","text":"hello","future":1}"#)
                .unwrap();
        assert_eq!(
            event,
            new_state_event(StateValue::Typing, None, "")
                .with_text("hello")
                .into()
        );
    }

    #[test]
    fn recording_duration_saturates_instead_of_wrapping() {
        let event = new_state_event(StateValue::Recording, Some(Duration::MAX), "");
        assert_eq!(event.recording_duration_ms, Some(i64::MAX));
    }
}
