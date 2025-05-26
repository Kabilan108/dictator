package cmd

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"

	"github.com/kabilan108/dictator/internal/audio"
	"github.com/kabilan108/dictator/internal/config"
	"github.com/kabilan108/dictator/internal/utils"
)

var cfgFile string

var rootCmd = &cobra.Command{
	Use:   "dictator",
	Short: "hello world",
	Long:  `hello world`,
}

var daemonCmd = &cobra.Command{
	Use:   "daemon",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		config.InitConfigFile()
		c, err := config.GetConfig()
		if err != nil {
			utils.Fatal("daemon", "failed to load config: %v", err)
		}

		Log := utils.NewLogger(c.App.LogLevel, "daemon")
		Log.D("daemon called")

		rec, err := audio.NewRecorder(c.Audio, c.App.LogLevel)
		if err != nil {
			utils.Fatal("daemon", "failed to create recorder: %v", err)
		}

		whispr := audio.NewWhisperClient(&c.API, c.App.LogLevel)

		if err := rec.Start(); err != nil {
			utils.Fatal("daemon", "failed to start recording: %v", err)
		}

		for range 10 {
			time.Sleep(1 * time.Second)
		}

		wav, path, err := rec.Stop()
		if err != nil {
			utils.Fatal("daemon", "failed to stop recording: %v", err)
		}

		wavFile, err := audio.WriteAudioData(path, wav)
		if err != nil {
			utils.Fatal("daemon", "failed to write audio to file: %v", err)
		}

		if err := rec.Close(); err != nil {
			Log.W("failed to stop PortAudio")
		}

		Log.I("recording saved to '%v'")

		ctx, _ := context.WithCancel(context.Background())
		req := audio.TranscriptionRequest{
			AudioData: wav,
			Filename: wavFile.Name(),
			Model: c.API.Model,
		}
		resp, err := whispr.Transcribe(ctx, &req)
		if err != nil {
			utils.Fatal("daemon", "failed to transcribe audio: %v", err)
		}
		Log.I("transcript: '%s'", resp.Text)
	},
}

var startCmd = &cobra.Command{
	Use:   "start",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := utils.NewLogger(utils.LevelDebug, "start")
		Log.D("start called")
	},
}

var stopCmd = &cobra.Command{
	Use:   "stop",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := utils.NewLogger(utils.LevelDebug, "stop")
		Log.D("stop called")
	},
}

var toggleCmd = &cobra.Command{
	Use:   "toggle",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := utils.NewLogger(utils.LevelDebug, "toggle")
		Log.D("toggle called")
	},
}

var cancelCmd = &cobra.Command{
	Use:   "cancel",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := utils.NewLogger(utils.LevelDebug, "cancel")
		Log.D("cancel called")
	},
}

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := utils.NewLogger(utils.LevelDebug, "status")
		Log.D("status called")
	},
}

// Execute adds all child commands to the root command and sets flags appropriately.
func Execute() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}

// initConfig reads in config file and ENV variables if set.
func initConfig() {
	if cfgFile != "" {
		viper.SetConfigFile(cfgFile)
	} else {
		home, err := os.UserHomeDir()
		cobra.CheckErr(err)

		viper.AddConfigPath(filepath.Join(home, ".config", "dictator"))
		viper.SetConfigName("config")
		viper.SetConfigType("json")
	}
	viper.AutomaticEnv() // read in environment variables that match

	if err := viper.ReadInConfig(); err == nil {
		fmt.Fprintln(os.Stderr, "using config file:", viper.ConfigFileUsed())
	}
}

func init() {
	cobra.OnInitialize(initConfig)

	rootCmd.AddCommand(daemonCmd)
	rootCmd.AddCommand(startCmd)
	rootCmd.AddCommand(stopCmd)
	rootCmd.AddCommand(toggleCmd)
	rootCmd.AddCommand(cancelCmd)
	rootCmd.AddCommand(statusCmd)

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default is $HOME/.config/dictator/config.json)")
	rootCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
