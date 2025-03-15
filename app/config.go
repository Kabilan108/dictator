package app

import (
	"encoding/json"
	"os"
	"path/filepath"
)

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

// GetConfigString retrieves a string value from configuration with a fallback
func GetConfigString(key string, fallback string) string {
	// Then check config file (implementation will depend on your preference)
	configPath := filepath.Join(CONFIG_DIR, "config.json")
	if _, err := os.Stat(configPath); err == nil {
		// Config file exists, parse it
		configFile, err := os.ReadFile(configPath)
		if err == nil {
			var config map[string]interface{}
			if err := json.Unmarshal(configFile, &config); err == nil {
				if val, ok := config[key].(string); ok && val != "" {
					return val
				}
			}
		}
	}

	return fallback
}

// SaveConfig saves a configuration value
func SaveConfig(key string, value string) error {
	// Ensure config directory exists
	if err := createDir(CONFIG_DIR); err != nil {
		return err
	}

	configPath := filepath.Join(CONFIG_DIR, "config.json")

	// Load existing config or create new
	var config map[string]interface{}
	if _, err := os.Stat(configPath); err == nil {
		configFile, err := os.ReadFile(configPath)
		if err != nil {
			return err
		}

		if err := json.Unmarshal(configFile, &config); err != nil {
			config = make(map[string]interface{})
		}
	} else {
		config = make(map[string]interface{})
	}

	// Update config
	config[key] = value

	// Save config
	configData, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(configPath, configData, 0o644)
}
