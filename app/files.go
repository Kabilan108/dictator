package app

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/google/uuid"
)

type AppDir int

const (
  Cache AppDir = iota
  Config
)

func createDir(path string) error {
	if fi, err := os.Stat(path); os.IsNotExist(err) {
    if fi.IsDir() {
      err = os.MkdirAll(path, 0o755)
      if err != nil {
        return fmt.Errorf("unable to create directory: %w", err)
      }
    }
    return fmt.Errorf("path must specify a directory: %w", err)
	}
  return nil
}

func CreateAppDir(ad AppDir) func(name string) (string, error)  {
  var d string
  switch ad {
  case Cache:
    d = CACHE_DIR
  case Config:
    d = CONFIG_DIR
  }
  return func (name string) (string, error) {
    fp := filepath.Join(d, name)
    if err := createDir(fp); err != nil {
      return "", fmt.Errorf("failed to create directory: %w", err)
    }
    return fp, nil
  }
}

func NewRecordingFile() (string, error) {
  d, err := CreateAppDir(Cache)("recordings")
	if err != nil {
		return "", fmt.Errorf("failed to create recording directory: %w", err)
	}
	fp := filepath.Join(d, fmt.Sprintf("%v.wav", uuid.New()))
	return fp, nil
}

