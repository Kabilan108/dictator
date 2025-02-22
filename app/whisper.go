package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
)

type WhisperServer struct {
	cmd        *exec.Cmd
	binary     string
	host       string
	logFile    *os.File
	modelPath  string
	port       string
	processors string
	threads    string
	pidFile    string
}

type WhisperResponse struct {
	Text string `json:"text"`
}

// TODO: add script to download model and set model path
// TODO: server setting should be configurable

func NewWhisperServer() (*WhisperServer, error) {
	bp := filepath.Join("bin", "whisper-server")
	if _, err := os.Stat(bp); os.IsNotExist(err) {
		return nil, fmt.Errorf("whisper-server binary not found at %s", bp)
	}

	pidDir, err := CreateAppDir(Config)("")
	if err != nil {
		return nil, err
	}

	return &WhisperServer{
		binary:     bp,
		host:       "127.0.0.1",
		modelPath:  "models/ggml-large-v3-turbo-q8_0.bin",
		port:       "3337",
		processors: "1",
		threads:    "4",
		pidFile:    filepath.Join(pidDir, ".whisper-server.pid"),
	}, nil
}

func (ws *WhisperServer) killExistingProcesses() {
	if _, err := os.Stat(ws.pidFile); !os.IsNotExist(err) {
		// read PID from file
		pidBytes, err := os.ReadFile(ws.pidFile)
		if err == nil {
			pid, err := strconv.Atoi(string(pidBytes))
			if err == nil {
				// attempt to kill process
				if proc, err := os.FindProcess(pid); err == nil {
					if err := proc.Kill(); err == nil {
						Log.I("Terminated existing whisper-server with PID %d", pid)
					} else {
						Log.E("Failed to kill existing whisper-server with PID %d: %v", pid, err)
					}
				}
			}
		}
		os.Remove(ws.pidFile)
	}
}

func (ws *WhisperServer) Start() error {
	ws.killExistingProcesses()

	// create log file
	lf, err := NewLogFile("whisper-server")
	if err != nil {
		return err
	}
	ws.logFile = lf

	// set up start command
	cmd := exec.Command(ws.binary,
		"-t", ws.threads, "-p", ws.processors, "-m", ws.modelPath, "--host", ws.host,
		"--port", ws.port,
	)
	cmd.Stdout = lf
	cmd.Stderr = lf

	// start the process
	if err := cmd.Start(); err != nil {
		lf.Close()
		return fmt.Errorf("failed to start whisper server: %w", err)
	}

	ws.cmd = cmd
	Log.I("Started whisper server with PID %d, logging to %s", cmd.Process.Pid, lf.Name())

	// write PID to file
	err = os.WriteFile(ws.pidFile, []byte(strconv.Itoa(cmd.Process.Pid)), 0644)
	if err != nil {
		Log.W("Failed to write PID file %s: %v", ws.pidFile, err)
	}

	// monitor the process
	go func() {
		err := cmd.Wait()
		if err != nil {
			Log.E("Whisper server exited with error: %v", err)
		} else {
			Log.I("Whisper server exited successfully")
		}
	}()

	return nil
}

func (ws *WhisperServer) Stop() error {
	if ws.cmd != nil && ws.cmd.Process != nil {
		if err := ws.cmd.Process.Kill(); err != nil {
			return fmt.Errorf("failed to stop whisper server: %w", err)
		}
	}
	if ws.logFile != nil {
		if err := ws.logFile.Close(); err != nil {
			return fmt.Errorf("failed to close log file: %w", err)
		}
	}
	return nil
}

func (ws *WhisperServer) Transcribe(fp string) (string, error) {
	f, err := os.Open(fp)
	if err != nil {
		return "", fmt.Errorf("failed to open file %s: %w", fp, err)
	}
	defer f.Close()

	// create buffer to store multipart form data
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)

	// add the file to the form data
	part, err := writer.CreateFormFile("file", filepath.Base(fp))
	if err != nil {
		return "", fmt.Errorf("failed to create form file: %w", err)
	}
	_, err = io.Copy(part, f)
	if err != nil {
		return "", fmt.Errorf("failed to copy file to form: %w", err)
	}

	// add response-format field
	err = writer.WriteField("response-format", "json")
	if err != nil {
		return "", fmt.Errorf("failed to add response-format field: %w", err)
	}

	if err := writer.Close(); err != nil {
		return "", fmt.Errorf("failed to close multipart writer: %w", err)
	}

	// create request
	url := fmt.Sprintf("http://%s:%s/inference", ws.host, ws.port)
	req, err := http.NewRequest("POST", url, &body)
	if err != nil {
		return "", fmt.Errorf("failed to create HTTP request: %w", err)
	}
	req.Header.Set("Content-Type", writer.FormDataContentType())

	// send request
	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("failed to send HTTP request: %w", err)
	}
	defer resp.Body.Close()

	// check response status
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("HTTP request failed with status code %d", resp.StatusCode)
	}

	// parse response
	var wr WhisperResponse
	err = json.NewDecoder(resp.Body).Decode(&wr)
	if err != nil {
		return "", fmt.Errorf("failed to decode JSON response: %w", err)
	}

	return wr.Text, nil
}
