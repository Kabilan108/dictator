package utils

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/viper"
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

type Config struct {
	API   APIConfig   `json:"api" mapstructure:"api"`
	App   AppConfig   `json:"app" mapstructure:"app"`
	Audio AudioConfig `json:"audio" mapstructure:"audio"`
}

type APIConfig struct {
	Endpoint   string `json:"endpoint" mapstructure:"endpoint"`
	Key        string `json:"key" mapstructure:"key"`
	Model      string `json:"model" mapstructure:"model"`
	TimeoutSec int    `json:"timeout" mapstructure:"timeout"`
}

type AudioConfig struct {
	SampleRate     int `json:"sample_rate" mapstructure:"sample_rate"`
	Channels       int `json:"channels" mapstructure:"channels"`
	BitDepth       int `json:"bit_depth" mapstructure:"bit_depth"`
	FramesPerBlock int `json:"frames_per_block" mapstructure:"frames_per_block"`
	MaxDurationMin int `json:"max_duration_min" mapstructure:"max_duration_min"`
}

type AppConfig struct {
	LogLevel        LogLevel `json:"log_level" mapstructure:"log_level"`
	MaxRecordingMin int      `json:"max_recording_min" mapstructure:"max_recording_seconds"`
	TypingDelayMS   int      `json:"typing_delay_ms" mapstructure:"typing_delay_ms"`
}

func DefaultConfig() *Config {
	return &Config{
		API: APIConfig{
			Endpoint:   "https://sietch.sole-pierce.ts.net/siren/v1/audio/transcriptions",
			Key:        "",
			Model:      "distil-large-v3",
			TimeoutSec: 60,
		},
		Audio: AudioConfig{
			SampleRate:     16000,
			Channels:       1,
			BitDepth:       16,
			FramesPerBlock: 1024,
			MaxDurationMin: 5,
		},
		App: AppConfig{
			TypingDelayMS:   10,
			MaxRecordingMin: 5,
			LogLevel:        LevelDebug,
		},
	}
}

func Load() (*Config, error) {
	config := DefaultConfig()

	viper.SetConfigName("config")
	viper.SetConfigType("json")
	viper.AddConfigPath(CONFIG_DIR)

	viper.SetEnvPrefix("DICTATOR")
	viper.AutomaticEnv()

	if err := viper.ReadInConfig(); err != nil {
		if _, ok := err.(viper.ConfigFileNotFoundError); ok {
			return config, nil
		}
		return nil, fmt.Errorf("failed to read config file: %w", err)
	}

	if err := viper.Unmarshal(config); err != nil {
		return nil, fmt.Errorf("failed to unmarshal config: %w", err)
	}

	if err := Validate(config); err != nil {
		return nil, fmt.Errorf("config validation failed: %w", err)
	}

	return config, nil
}

func Validate(config *Config) error {
	if config.API.Endpoint == "" {
		return fmt.Errorf("API endpoint is required")
	}
	if config.API.Key == "" {
		return fmt.Errorf("API key is required")
	}
	if config.API.TimeoutSec <= 0 {
		return fmt.Errorf("API timeout must be > 0")
	}

	if config.Audio.SampleRate <= 0 {
		return fmt.Errorf("audio sample rate must be positive")
	}
	if config.Audio.Channels <= 0 {
		return fmt.Errorf("audio channels must be positive")
	}
	if config.Audio.BitDepth <= 0 {
		return fmt.Errorf("audio bit depth must be positive")
	}
	if config.Audio.FramesPerBlock <= 0 {
		return fmt.Errorf("audio frames per block must be positive")
	}
	if config.Audio.MaxDurationMin <= 0 {
		return fmt.Errorf("audio max duration min must be positive")
	}

	if config.App.MaxRecordingMin <= 0 {
		return fmt.Errorf("max recording minutes must be positive")
	}
	if config.App.TypingDelayMS < 0 {
		return fmt.Errorf("typing delay cannot be negative")
	}

	return nil
}

func InitConfigFile() error {
	configPath := filepath.Join(CONFIG_DIR, "config.json")

	if _, err := os.Stat(configPath); err == nil {
		return nil
	}

	if err := os.MkdirAll(CONFIG_DIR, 0o755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	defaultConfig := DefaultConfig()

	configData, err := json.MarshalIndent(defaultConfig, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal default config: %w", err)
	}

	if err := os.WriteFile(configPath, configData, 0o644); err != nil {
		return fmt.Errorf("failed to write config file: %w", err)
	}

	return nil
}

var globalConfig *Config

func GetConfig() (*Config, error) {
	if globalConfig == nil {
		config, err := Load()
		if err != nil {
			return nil, err
		}
		globalConfig = config
	}
	return globalConfig, nil
}
