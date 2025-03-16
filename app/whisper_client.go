package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"os"
	"path/filepath"
	"time"
)

// handles communication with openai compatible apis
type WhisperClient struct {
	ApiUrl       string
	ApiKey       string
	DefaultModel string
	Theme        string
}

type WhisperResponse struct {
	Text string `json:"text"`
}

type ModelInfo struct {
	ID string `json:"id"`
}

type ModelsResponse struct {
	Data []ModelInfo `json:"data"`
}

func NewWhisperClient() (*WhisperClient, error) {
	c := LoadConfig()
	client := &WhisperClient{
		ApiUrl:       c.ApiUrl,
		ApiKey:       c.ApiKey,
		DefaultModel: c.DefaultModel,
		Theme:        c.Theme,
	}
	return client, nil
}

// send audio file to the api and return the transcription
func (wc *WhisperClient) Transcribe(fp string) (string, error) {
	f, err := os.Open(fp)
	if err != nil {
		return "", fmt.Errorf("failed to open file %s: %w", fp, err)
	}
	defer f.Close()

	// Create multipart form data
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)

	// Add the file to the form data
	part, err := writer.CreateFormFile("file", filepath.Base(fp))
	if err != nil {
		return "", fmt.Errorf("failed to create form file: %w", err)
	}
	_, err = io.Copy(part, f)
	if err != nil {
		return "", fmt.Errorf("failed to copy file to form: %w", err)
	}

	// Add model field if configured
	if wc.DefaultModel != "" {
		err = writer.WriteField("model", wc.DefaultModel)
		if err != nil {
			return "", fmt.Errorf("failed to add model field: %w", err)
		}
	}

	if err := writer.Close(); err != nil {
		return "", fmt.Errorf("failed to close multipart writer: %w", err)
	}

	// Create request to OpenAI-compatible endpoint
	url := fmt.Sprintf("%s/v1/audio/transcriptions", wc.ApiUrl)
	req, err := http.NewRequest("POST", url, &body)
	if err != nil {
		return "", fmt.Errorf("failed to create HTTP request: %w", err)
	}

	req.Header.Set("Content-Type", writer.FormDataContentType())

	// Add API key if provided
	if wc.ApiKey != "" {
		req.Header.Set("Authorization", fmt.Sprintf("Bearer %s", wc.ApiKey))
	}

	// Send request
	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("failed to send HTTP request: %w", err)
	}
	defer resp.Body.Close()

	// Check response status
	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		return "", fmt.Errorf("HTTP request failed with status code %d: %s", resp.StatusCode, string(bodyBytes))
	}

	// Parse response
	var wr WhisperResponse
	err = json.NewDecoder(resp.Body).Decode(&wr)
	if err != nil {
		return "", fmt.Errorf("failed to decode JSON response: %w", err)
	}

	return wr.Text, nil
}

// check if api supports /v1/models endpoint
func (wc *WhisperClient) SupportsModelsEndpoint() bool {
	url := fmt.Sprintf("%s/v1/models", wc.ApiUrl)

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return false
	}

	if wc.ApiKey != "" {
		req.Header.Set("Authorization", fmt.Sprintf("Bearer %s", wc.ApiKey))
	}

	client := &http.Client{
		Timeout: 5 * time.Second,
	}

	resp, err := client.Do(req)
	if err != nil {
		return false
	}
	defer resp.Body.Close()

	return resp.StatusCode == http.StatusOK
}

func (wc *WhisperClient) ListModels() ([]ModelInfo, error) {
	if !wc.SupportsModelsEndpoint() {
		return nil, fmt.Errorf("model listing not supported by this API")
	}

	url := fmt.Sprintf("%s/v1/models", wc.ApiUrl)

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create HTTP request: %w", err)
	}

	if wc.ApiKey != "" {
		req.Header.Set("Authorization", fmt.Sprintf("Bearer %s", wc.ApiKey))
	}

	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to sent HTTP request: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP request failed with status code %d", resp.StatusCode)
	}

	var modelsResp ModelsResponse
	err = json.NewDecoder(resp.Body).Decode(&modelsResp)
	if err != nil {
		return nil, fmt.Errorf("failed to decode JSON response: %w", err)
	}

	return modelsResp.Data, nil
}
