package app

import (
	"encoding/json"
	"log"
	"os"
	"path/filepath"
)

type DictatorConfig struct {
	ApiUrl       string `json:"apiUrl"`
	ApiKey       string `json:"apiKey"`
	DefaultModel string `json:"defaultModel"`
	Theme        string `json:"theme"`
}

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

var defaultConfig = DictatorConfig{
	ApiUrl:       "http://localhost:9934",
	ApiKey:       "",
	DefaultModel: "",
	Theme:        "catppuccinMocha",
}

func LoadConfig() DictatorConfig {
	configPath := filepath.Join(CONFIG_DIR, "config.json")
	config := defaultConfig

	if _, err := os.Stat(configPath); err == nil {
		configFile, err := os.ReadFile(configPath)
		if err == nil {
			err = json.Unmarshal(configFile, &config)
			if err != nil {
				log.Printf("Error parsing config file: %v", err)
			}
		} else {
			log.Printf("Error reading config file: %v", err)
		}
	}

	return config
}

func SaveConfig(config DictatorConfig) error {
	configPath := filepath.Join(CONFIG_DIR, "config.json")

	// Ensure directory exists
	err := os.MkdirAll(CONFIG_DIR, 0o755)
	if err != nil {
		return err
	}

	// Marshal config to JSON
	data, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return err
	}

	// Write to file
	return os.WriteFile(configPath, data, 0o644)
}
