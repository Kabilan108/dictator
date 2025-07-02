package audio

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"mime/multipart"
	"net/http"
	"strings"
	"time"

	"github.com/kabilan108/dictator/internal/utils"
)

type TranscriptionRequest struct {
	AudioData []byte
	Filename  string
	Model     string // optional, defaults to "distil-large-v3"
	Language  string // optional
}

type TranscriptionResponse struct {
	Text string `json:"text"`
}

type WhisperClient interface {
	Transcribe(ctx context.Context, req *TranscriptionRequest) (*TranscriptionResponse, error)
}

type whisperClient struct {
	config     *utils.APIConfig
	httpClient *http.Client
}

func NewWhisperClient(c *utils.APIConfig, logLevel string) WhisperClient {
	timeout := time.Duration(c.Timeout) * time.Second
	return &whisperClient{
		config:     c,
		httpClient: &http.Client{Timeout: timeout},
	}
}

func (c *whisperClient) Transcribe(ctx context.Context, req *TranscriptionRequest) (*TranscriptionResponse, error) {
	slog.Debug("starting transcription request", "filename", req.Filename)

	activeProvider, exists := c.config.Providers[c.config.ActiveProvider]
	if !exists {
		return nil, fmt.Errorf("active provider '%s' not found", c.config.ActiveProvider)
	}

	if activeProvider.Key == "" {
		return nil, fmt.Errorf("API key is required but not configured for provider '%s'", c.config.ActiveProvider)
	}

	// create multipart form data
	body := &bytes.Buffer{}
	writer := multipart.NewWriter(body)

	fileWriter, err := writer.CreateFormFile("file", req.Filename)
	if err != nil {
		slog.Error("failed to create form file", "err", err)
		return nil, fmt.Errorf("failed to create form file: %w", err)
	}

	if _, err := fileWriter.Write(req.AudioData); err != nil {
		slog.Error("failed to write audio data", "err", err)
		return nil, fmt.Errorf("failed to write audio data: %w", err)
	}

	model := req.Model
	if model == "" {
		model = activeProvider.Model
	}
	if model == "" {
		model = "distil-large-v3" // Final fallback
	}
	if err := writer.WriteField("model", model); err != nil {
		return nil, fmt.Errorf("failed to write model field: %w", err)
	}

	if req.Language != "" {
		if err := writer.WriteField("language", req.Language); err != nil {
			return nil, fmt.Errorf("failed to write language field: %w", err)
		}
	}

	if err := writer.Close(); err != nil {
		return nil, fmt.Errorf("failed to close multipart writer: %w", err)
	}

	url := activeProvider.Endpoint
	if !strings.HasSuffix(url, "/transcriptions") {
		if strings.HasSuffix(url, "/v1/audio/transcriptions") {
			// already complete
		} else if strings.HasSuffix(url, "/v1/audio") {
			url += "/transcriptions"
		} else if strings.HasSuffix(url, "/v1") {
			url += "/audio/transcriptions"
		} else {
			url += "/v1/audio/transcriptions"
		}
	}

	httpReq, err := http.NewRequestWithContext(ctx, "POST", url, body)
	if err != nil {
		slog.Error("failed to create http request", "err", err)
		return nil, fmt.Errorf("failed to create HTTP request: %w", err)
	}

	httpReq.Header.Set("Authorization", "Bearer "+activeProvider.Key)
	httpReq.Header.Set("Content-Type", writer.FormDataContentType())

	slog.Debug("sending request", "url", url, "model", model)

	var resp *http.Response
	var lastErr error

	for attempt := range 2 {
		resp, err = c.httpClient.Do(httpReq)
		if err != nil {
			lastErr = err
			if attempt == 0 {
				slog.Warn("request attempt failed, retrying", "attempt", attempt+1, "err", err)
				time.Sleep(1 * time.Second)
				continue
			}
		} else {
			break
		}
	}

	if resp == nil {
		slog.Error("all request attempts failed", "err", lastErr)
		return nil, fmt.Errorf("request failed after 2 attempts: %w", lastErr)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		errorMsg := fmt.Sprintf("API request failed with status %d: %s", resp.StatusCode, string(bodyBytes))
		slog.Error("api request failed", "err", errorMsg)
		return nil, errors.New(errorMsg)
	}

	// Parse JSON response
	var transcriptionResp TranscriptionResponse
	if err := json.NewDecoder(resp.Body).Decode(&transcriptionResp); err != nil {
		slog.Error("failed to decode response", "err", err)
		return nil, fmt.Errorf("failed to decode response: %w", err)
	}

	slog.Debug("transcription completed successfully", "length", len(transcriptionResp.Text))
	return &transcriptionResp, nil
}
