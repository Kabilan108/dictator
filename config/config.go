package config

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/viper"
)

// Config holds all configuration for the application
type Config struct {
	App     AppConfig     `mapstructure:"app"`
	Audio   AudioConfig   `mapstructure:"audio"`
	Whisper WhisperConfig `mapstructure:"whisper"`
}

// AppConfig holds application-specific configuration
type AppConfig struct {
	Width             int    `mapstructure:"width"`
	Height            int    `mapstructure:"height"`
	MinWidth          int    `mapstructure:"min_width"`
	MaxWidth          int    `mapstructure:"max_width"`
	MinHeight         int    `mapstructure:"min_height"`
	MaxHeight         int    `mapstructure:"max_height"`
	Title             string `mapstructure:"title"`
	DisableResize     bool   `mapstructure:"disable_resize"`
	HideWindowOnClose bool   `mapstructure:"hide_window_on_close"`
}

// AudioConfig holds audio recording configuration
type AudioConfig struct {
	SampleRate     int `mapstructure:"sample_rate"`
	FramesPerBlock int `mapstructure:"frames_per_block"`
	InputChannels  int `mapstructure:"input_channels"`
}

// WhisperConfig holds whisper.cpp server configuration
type WhisperConfig struct {
	ModelPath   string  `mapstructure:"model_path"`
	Host        string  `mapstructure:"host"`
	Port        int     `mapstructure:"port"`
	Threads     int     `mapstructure:"threads"`
	Language    string  `mapstructure:"language"`
	UseRemote   bool    `mapstructure:"use_remote"`
	RemoteURL   string  `mapstructure:"remote_url"`
	TranslateEn bool    `mapstructure:"translate_en"`
	WordThold   float64 `mapstructure:"word_thold"`
	BestOf      int     `mapstructure:"best_of"`
}

var (
	// Cfg is the global configuration instance
	Cfg *Config
	// v is the viper instance
	v *viper.Viper
)

// Initialize sets up the configuration system
func Initialize() error {
	v = viper.New()

	// Set default configuration values
	setDefaults()

	// Get config directory
	configDir, err := getConfigDir()
	if err != nil {
		return fmt.Errorf("failed to get config directory: %w", err)
	}

	// Ensure config directory exists
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	configFile := filepath.Join(configDir, "config.yaml")

	// If config file doesn't exist, create it with defaults
	if _, err := os.Stat(configFile); os.IsNotExist(err) {
		if err := v.SafeWriteConfigAs(configFile); err != nil {
			return fmt.Errorf("failed to write default config: %w", err)
		}
	}

	// Set config file path and type
	v.SetConfigFile(configFile)
	v.SetConfigType("yaml")

	// Read config file
	if err := v.ReadInConfig(); err != nil {
		return fmt.Errorf("failed to read config file: %w", err)
	}

	// Unmarshal config into struct
	Cfg = &Config{}
	if err := v.Unmarshal(Cfg); err != nil {
		return fmt.Errorf("failed to unmarshal config: %w", err)
	}

	return nil
}

// Save persists the current configuration to disk
func Save() error {
	return v.WriteConfig()
}

// setDefaults sets the default configuration values
func setDefaults() {
	// App defaults
	v.SetDefault("app.width", 500)
	v.SetDefault("app.height", 150)
	v.SetDefault("app.min_width", 500)
	v.SetDefault("app.max_width", 500)
	v.SetDefault("app.min_height", 150)
	v.SetDefault("app.max_height", 600)
	v.SetDefault("app.title", "dictator")
	v.SetDefault("app.disable_resize", false)
	v.SetDefault("app.hide_window_on_close", false)

	// Audio defaults
	v.SetDefault("audio.sample_rate", 44100)
	v.SetDefault("audio.frames_per_block", 1024)
	v.SetDefault("audio.input_channels", 1)

	// Whisper defaults
	v.SetDefault("whisper.model_path", "models/ggml-base.en.bin")
	v.SetDefault("whisper.host", "127.0.0.1")
	v.SetDefault("whisper.port", 8080)
	v.SetDefault("whisper.threads", 4)
	v.SetDefault("whisper.language", "en")
	v.SetDefault("whisper.use_remote", false)
	v.SetDefault("whisper.remote_url", "")
	v.SetDefault("whisper.translate_en", false)
	v.SetDefault("whisper.word_thold", 0.01)
	v.SetDefault("whisper.best_of", 2)
}

// getConfigDir returns the configuration directory following XDG spec
func getConfigDir() (string, error) {
	configHome := os.Getenv("XDG_CONFIG_HOME")
	if configHome == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("failed to get user home directory: %w", err)
		}
		configHome = filepath.Join(home, ".config")
	}
	return filepath.Join(configHome, "dictator"), nil
}
