package app

import (
	"os"
	"path/filepath"
)

// TODO: drive these from env vars / config file
const API_KEY    = "secret_api_key"

var CONFIG_DIR = func() string {
	dir, err := os.UserConfigDir()
	if err != nil {
		return filepath.Join(os.Getenv("HOME"), ".config", "dictator")
	}
	return filepath.Join(dir, "dictator")
}()

var CACHE_DIR = func() string {
	dir, err := os.UserCacheDir()
	if err != nil {
		return filepath.Join(os.Getenv("HOME"), ".cache", "dictator")
	}
	return filepath.Join(dir, "dictator")
}()

