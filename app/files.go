package app

import (
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type AppDir int

const (
	CacheDir AppDir = iota
	ConfigDir
)

func createDir(path string) error {
	if _, err := os.Stat(path); os.IsNotExist(err) {
		err = os.MkdirAll(path, 0o755)
		if err != nil {
			return fmt.Errorf("unable to create directory: %w", err)
		}
	}
	return nil
}

func CreateAppDir(ad AppDir) func(name string) (string, error) {
	var d string
	switch ad {
	case CacheDir:
		d = CACHE_DIR
	case ConfigDir:
		d = CONFIG_DIR
	}
	return func(name string) (string, error) {
		fp := filepath.Join(d, name)
		if err := createDir(fp); err != nil {
			return "", fmt.Errorf("failed to create directory: %w", err)
		}
		return fp, nil
	}
}

func NewRecordingFile() (string, error) {
	d, err := CreateAppDir(CacheDir)("recordings")
	if err != nil {
		return "", fmt.Errorf("failed to create recording directory: %w", err)
	}
	now := time.Now().Format("01022006-150405")
	fp := filepath.Join(d, fmt.Sprintf("%v.wav", now))
	return fp, nil
}

func NewLogFile(prefix string) (*os.File, error) {
	d, err := CreateAppDir(ConfigDir)("logs")
	if err != nil {
		return nil, err
	}

	now := time.Now().Format("01022006-150405")
	fp := filepath.Join(d, fmt.Sprintf("%v-%v.log", prefix, now))

	f, err := os.OpenFile(fp, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return nil, err
	}

	return f, nil
}
