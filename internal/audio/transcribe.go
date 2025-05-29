package audio

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
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
	log        utils.Logger
}

func NewWhisperClient(c *utils.APIConfig, l utils.LogLevel) WhisperClient {
	timeout := time.Duration(c.TimeoutSec) * time.Second
	return &whisperClient{
		config:     c,
		httpClient: &http.Client{Timeout: timeout},
		log:        utils.NewLogger(l, "whisper"),
	}
}

func (c *whisperClient) Transcribe(ctx context.Context, req *TranscriptionRequest) (*TranscriptionResponse, error) {
	c.log.D("starting transcription request for file: %s", req.Filename)

	if c.config.Key == "" {
		return nil, fmt.Errorf("API key is required but not configured")
	}

	// create multipart form data
	body := &bytes.Buffer{}
	writer := multipart.NewWriter(body)

	fileWriter, err := writer.CreateFormFile("file", req.Filename)
	if err != nil {
		c.log.E("failed to create form file: %v", err)
		return nil, fmt.Errorf("failed to create form file: %w", err)
	}

	if _, err := fileWriter.Write(req.AudioData); err != nil {
		c.log.E(fmt.Sprintf("failed to write audio data: %v", err))
		return nil, fmt.Errorf("failed to write audio data: %w", err)
	}

	model := req.Model
	if model == "" {
		model = c.config.Model
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

	url := c.config.Endpoint
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
		c.log.E("failed to create http request: %v", err)
		return nil, fmt.Errorf("failed to create HTTP request: %w", err)
	}

	httpReq.Header.Set("Authorization", "Bearer "+c.config.Key)
	httpReq.Header.Set("Content-Type", writer.FormDataContentType())

	c.log.D("sending request to: %s with model: %s", url, model)

	var resp *http.Response
	var lastErr error

	for attempt := range 2 {
		resp, err = c.httpClient.Do(httpReq)
		if err != nil {
			lastErr = err
			if attempt == 0 {
				c.log.W("request attempt %d failed, retrying: %v", attempt+1, err)
				time.Sleep(1 * time.Second)
				continue
			}
		} else {
			break
		}
	}

	if resp == nil {
		c.log.E("all request attempts failed: %v", lastErr)
		return nil, fmt.Errorf("request failed after 2 attempts: %w", lastErr)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		errorMsg := fmt.Sprintf("API request failed with status %d: %s", resp.StatusCode, string(bodyBytes))
		c.log.E(errorMsg)
		return nil, errors.New(errorMsg)
	}

	// Parse JSON response
	var transcriptionResp TranscriptionResponse
	if err := json.NewDecoder(resp.Body).Decode(&transcriptionResp); err != nil {
		c.log.E(fmt.Sprintf("Failed to decode JSON response: %v", err))
		return nil, fmt.Errorf("failed to decode response: %w", err)
	}

	c.log.D("transcription completed successfully, text length: %d characters", len(transcriptionResp.Text))

	return &transcriptionResp, nil
}
