/*
Copyright © 2025 kabilan108 tonykabilanokeke@gmail.com
*/

package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"

	"github.com/kabilan108/dictator/internal/daemon"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/utils"
)

var cfgFile string

var rootCmd = &cobra.Command{
	Use:   "dictator",
	Short: "whisper typing daemon for linux",
	Long: `dictator is a voice typing daemon for linux that enables voice typing anywhere
the cursor is positioned. the system uses a cli/daemon architecture where a single
binary operates in two modes:

- daemon mode: background service handling audio recording, transcription, and typing
- client mode: cli commands that communicate with the daemon via unix socket ipc

start the daemon with 'dictator daemon' then use commands like 'start', 'stop',
'toggle', 'cancel', and 'status' to control voice recording and transcription.`,
}

var daemonCmd = func(c *utils.Config) *cobra.Command {
	return &cobra.Command{
		Use:   "daemon",
		Short: "run the dictator daemon",
		Long:  `starts the dictator daemon in the foreground, listening for voice commands via ipc`,
		Run: func(cmd *cobra.Command, args []string) {
			d, err := daemon.NewDaemon(c)
			if err != nil {
				fmt.Fprintf(os.Stderr, "failed to create daemon: %v\n", err)
				os.Exit(1)
			}
			if err := d.Run(); err != nil {
				fmt.Fprintf(os.Stderr, "daemon exited with error: %v\n", err)
				os.Exit(1)
			}
		},
	}
}

var startCmd = func(lvl utils.LogLevel) *cobra.Command {
	return &cobra.Command{
		Use:   "start",
		Short: "start voice recording",
		Long:  `tells the daemon to start recording voice input`,
		Run: func(cmd *cobra.Command, args []string) {
			client := ipc.NewClient(lvl)
			ctx := context.Background()

			response, err := client.Start(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "Error: Cannot connect to daemon. Is it running?\n")
				fmt.Fprintf(os.Stderr, "Try starting the daemon with: dictator daemon\n")
				os.Exit(1)
			}

			if response.Success {
				fmt.Println("Recording started")
			} else {
				fmt.Fprintf(os.Stderr, "start command failed: %s\n", response.Error)
				os.Exit(1)
			}
		},
	}
}

var stopCmd = func(lvl utils.LogLevel) *cobra.Command {
	return &cobra.Command{
		Use:   "stop",
		Short: "stop voice recording and transcribe",
		Long:  `tells the daemon to stop recording and start transcription`,
		Run: func(cmd *cobra.Command, args []string) {
			client := ipc.NewClient(lvl)
			ctx := context.Background()

			response, err := client.Stop(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "Error: Cannot connect to daemon. Is it running?\n")
				fmt.Fprintf(os.Stderr, "Try starting the daemon with: dictator daemon\n")
				os.Exit(1)
			}

			if response.Success {
				fmt.Println("Recording stopped, transcribing...")
			} else {
				fmt.Fprintf(os.Stderr, "stop command failed: %s\n", response.Error)
				os.Exit(1)
			}
		},
	}
}

var toggleCmd = func(lvl utils.LogLevel) *cobra.Command {
	return &cobra.Command{
		Use:   "toggle",
		Short: "toggle voice recording",
		Long:  `toggles between starting and stopping voice recording`,
		Run: func(cmd *cobra.Command, args []string) {
			client := ipc.NewClient(lvl)
			ctx := context.Background()

			response, err := client.Toggle(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "cannot connect to daemon. is it running?\n")
				fmt.Fprintf(os.Stderr, "try starting the daemon with: dictator daemon\n")
				os.Exit(1)
			}

			if response.Success {
				state := response.Data[ipc.DataKeyState]
				switch state {
				case "recording":
					fmt.Println("recording started")
				case "idle":
					fmt.Println("recording stopped, transcribing...")
				default:
					fmt.Printf("state changed to: %s\n", state)
				}
			} else {
				fmt.Fprintf(os.Stderr, "toggle command failed: %s\n", response.Error)
				os.Exit(1)
			}
		},
	}
}

var cancelCmd = func(lvl utils.LogLevel) *cobra.Command {
	return &cobra.Command{
		Use:   "cancel",
		Short: "cancel current operation",
		Long:  `cancels any current recording or transcription operation`,
		Run: func(cmd *cobra.Command, args []string) {
			client := ipc.NewClient(lvl)
			ctx := context.Background()

			response, err := client.Cancel(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "cannot connect to daemon. is it running?\n")
				fmt.Fprintf(os.Stderr, "try starting the daemon with: dictator daemon\n")
				os.Exit(1)
			}

			if response.Success {
				fmt.Println("operation canceled")
			} else {
				fmt.Fprintf(os.Stderr, "cancel command failed: %s", response.Error)
				os.Exit(1)
			}
		},
	}
}

var statusCmd = func(lvl utils.LogLevel) *cobra.Command {
	return &cobra.Command{
		Use:   "status",
		Short: "get daemon status",
		Long:  `shows the current status of the dictator daemon`,
		Run: func(cmd *cobra.Command, args []string) {
			client := ipc.NewClient(lvl)
			ctx := context.Background()

			response, err := client.Status(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "cannot connect to daemon. is it running?\n")
				fmt.Fprintf(os.Stderr, "try starting the daemon with: dictator daemon\n")
				os.Exit(1)
			}

			if response.Success {
				fmt.Printf("daemon status:\n")
				fmt.Printf("  state:  %s\n", response.Data[ipc.DataKeyState])
				fmt.Printf("  uptime: %s\n", response.Data[ipc.DataKeyUptime])

				if duration, ok := response.Data[ipc.DataKeyRecordingDuration]; ok {
					fmt.Printf("  recording duration: %s\n", duration)
				}
				if lastError, ok := response.Data[ipc.DataKeyLastError]; ok {
					fmt.Printf("  last error: %s\n", lastError)
				}
			} else {
				fmt.Fprintf(os.Stderr, "status command failed: %s", response.Error)
				os.Exit(1)
			}
		},
	}
}

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
	if err := viper.ReadInConfig(); err != nil {
		cobra.CheckErr(err)
	}
	if err := utils.InitConfigFile(); err != nil {
		cobra.CheckErr(err)
	}
}

func init() {
	cobra.OnInitialize(initConfig)

	config, err := utils.GetConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to load config: %v", err)
		os.Exit(1)
	}

	rootCmd.AddCommand(daemonCmd(config))
	rootCmd.AddCommand(startCmd(config.App.LogLevel))
	rootCmd.AddCommand(stopCmd(config.App.LogLevel))
	rootCmd.AddCommand(toggleCmd(config.App.LogLevel))
	rootCmd.AddCommand(cancelCmd(config.App.LogLevel))
	rootCmd.AddCommand(statusCmd(config.App.LogLevel))

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default is $HOME/.config/dictator/utils.json)")
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
