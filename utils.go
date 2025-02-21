package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/google/uuid"
)

func createParentDir(path string) error {
	dir := filepath.Dir(path)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		err = os.MkdirAll(dir, 0o755)
		if err != nil {
			return fmt.Errorf("Unable to create directory: %w", err)
		}
	}
	return nil
}

func newRecordingFile() (string, error) {
	fp := filepath.Join(DATA_DIR, "recordings", fmt.Sprintf("%v.wav", uuid.New()))
  if err := createParentDir(fp); err != nil {
    return "", fmt.Errorf("Could not create recording directory: %w", err)
  }
  return fp, nil
}
