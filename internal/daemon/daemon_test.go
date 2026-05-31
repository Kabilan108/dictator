package daemon

import (
	"testing"

	"github.com/kabilan108/dictator/internal/ipc"
)

func TestPublicOSDErrorMessageSanitizesDetails(t *testing.T) {
	tests := map[string]string{
		ipc.ErrRecordingFailed + ": /tmp/private/audio.wav":                   ipc.ErrRecordingFailed,
		ipc.ErrTranscriptionFailed + ": API request failed with token abc123": ipc.ErrTranscriptionFailed,
		ipc.ErrTypingFailed + ": clipboard command failed":                    ipc.ErrTypingFailed,
		"unexpected path /home/user/private":                                  "dictation failed",
	}

	for input, want := range tests {
		if got := publicOSDErrorMessage(input); got != want {
			t.Fatalf("publicOSDErrorMessage(%q) = %q, want %q", input, got, want)
		}
	}
}
