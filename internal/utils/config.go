package utils

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"github.com/spf13/viper"
)

func init() {
	if err := os.MkdirAll(DATA_DIR, 0o755); err != nil {
		fmt.Fprintf(os.Stderr, "failed to create data dir: %v\n", err)
	}
	if err := os.MkdirAll(STATE_DIR, 0o755); err != nil {
		fmt.Fprintf(os.Stderr, "failed to create data dir: %v\n", err)
	}
}

func getAppDir(env, fallback string) string {
	if xdg := os.Getenv(env); xdg != "" {
		return filepath.Join(xdg, "dictator")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(os.Getenv("HOME"), ".local", fallback, "dictator")
	}
	return filepath.Join(home, ".local", fallback, "dictator")
}

var DATA_DIR = getAppDir("XDG_DATA_HOME", "share")
var STATE_DIR = getAppDir("XDG_STATE_HOME", "state")

var CONFIG_DIR = func() string {
	dir, err := os.UserConfigDir()
	if err != nil {
		return filepath.Join(os.Getenv("HOME"), ".config", "dictator")
	}
	return filepath.Join(dir, "dictator")
}()

type Config struct {
	API   APIConfig   `json:"api" mapstructure:"api"`
	Audio AudioConfig `json:"audio" mapstructure:"audio"`
}

type Provider struct {
	Endpoint string `json:"endpoint" mapstructure:"endpoint"`
	Key      string `json:"key" mapstructure:"key"`
	Model    string `json:"model" mapstructure:"model"`
}

type APIConfig struct {
	ActiveProvider string              `json:"active_provider" mapstructure:"active_provider"`
	Timeout        int                 `json:"timeout" mapstructure:"timeout"`
	Providers      map[string]Provider `json:"providers" mapstructure:"providers"`
}

type AudioConfig struct {
	SampleRate     int `json:"sample_rate" mapstructure:"sample_rate"`
	Channels       int `json:"channels" mapstructure:"channels"`
	BitDepth       int `json:"bit_depth" mapstructure:"bit_depth"`
	FramesPerBlock int `json:"frames_per_block" mapstructure:"frames_per_block"`
	MaxDurationMin int `json:"max_duration_min" mapstructure:"max_duration_min"`
}

func DefaultConfig() *Config {
	return &Config{
		API: APIConfig{
			ActiveProvider: "openai",
			Timeout:        60,
			Providers: map[string]Provider{
				"openai": {
					Endpoint: "https://api.openai.com/v1/audio/transcriptions",
					Key:      "",
					Model:    "gpt-4o-transcribe",
				},
			},
		},
		Audio: AudioConfig{
			SampleRate:     16000,
			Channels:       1,
			BitDepth:       16,
			FramesPerBlock: 1024,
			MaxDurationMin: 5,
		},
	}
}

func Validate(config *Config) error {
	if config.API.ActiveProvider == "" {
		return fmt.Errorf("active provider is required")
	}

	activeProvider, exists := config.API.Providers[config.API.ActiveProvider]
	if !exists {
		return fmt.Errorf("active provider '%s' not found in providers", config.API.ActiveProvider)
	}

	if activeProvider.Endpoint == "" {
		return fmt.Errorf("endpoint is required for active provider '%s'", config.API.ActiveProvider)
	}
	if activeProvider.Key == "" {
		return fmt.Errorf("API key is required for active provider '%s'", config.API.ActiveProvider)
	}
	if config.API.Timeout <= 0 {
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

	return nil
}

var globalConfig *Config

func GetConfig() (*Config, error) {
	viper.SetConfigName("config")
	viper.SetConfigType("json")
	viper.AddConfigPath(CONFIG_DIR)

	viper.SetEnvPrefix("DICTATOR")
	viper.AutomaticEnv()

	var once sync.Once
	var loadErr error

	once.Do(func() {
		// seed with defaults so partial configs/env vars merge correctly
		config := DefaultConfig()

		if err := viper.ReadInConfig(); err != nil {
			// if config file is missing, continue so env vars can still apply
			if _, ok := err.(viper.ConfigFileNotFoundError); !ok {
				loadErr = fmt.Errorf("config: %v", err)
				return
			}
		}

		if err := viper.Unmarshal(config); err != nil {
			loadErr = fmt.Errorf("config: failed to parse: %v", err)
			return
		}

		if err := Validate(config); err != nil {
			loadErr = fmt.Errorf("config: failed to validate: %v", err)
			return
		}

		globalConfig = config
	})

	return globalConfig, loadErr
}
