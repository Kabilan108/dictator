/*
Copyright © 2025 kabilan108 tonykabilanokeke@gmail.com
*/

// BUG: need to separate xdotool input from real keyboard input
//      right now dictator is typing and the user hits the fn key to, for example pause music,
//      that will trigger `fn+ [key]` and similarly with other modifiers

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"

	"github.com/kabilan108/dictator/internal/daemon"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/storage"
	"github.com/kabilan108/dictator/internal/typing"
	"github.com/kabilan108/dictator/internal/utils"
)

var (
	cfgFile, logLevel string
	version           = "dev"
)

var rootCmd = &cobra.Command{
	Use:   "dictator",
	Short: "whisper typing daemon for linux",
	Long: `dictator is a voice typing daemon for linux that enables voice typing.

start the daemon with 'dictator daemon' then use commands like 'start', 'stop',
'toggle', 'cancel', and 'status' to control voice recording and transcription.`,
}

var createDaemonCmd = func(c *utils.Config) *cobra.Command {
	return &cobra.Command{
		Use:   "daemon",
		Short: "run the dictator daemon",
		Long:  `starts the dictator daemon in the foreground, listening for voice commands via ipc`,
		Run: func(cmd *cobra.Command, args []string) {
			d, err := daemon.NewDaemon(c, logLevel)
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

var startCmd = &cobra.Command{
	Use:   "start",
	Short: "start voice recording",
	Long:  `tells the daemon to start recording voice input`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient(logLevel)
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

var stopCmd = &cobra.Command{
	Use:   "stop",
	Short: "stop voice recording and transcribe",
	Long:  `tells the daemon to stop recording and start transcription`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient(logLevel)
		ctx := context.Background()

		response, err := client.Stop(ctx)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error: Cannot connect to daemon. Is it running?\n")
			fmt.Fprintf(os.Stderr, "Try starting the daemon with: dictator daemon\n")
			os.Exit(1)
		}

		if response.Success {
			fmt.Println("Recording stopped, transcribing")
		} else {
			fmt.Fprintf(os.Stderr, "stop command failed: %s\n", response.Error)
			os.Exit(1)
		}
	},
}

var toggleCmd = &cobra.Command{
	Use:   "toggle",
	Short: "toggle voice recording",
	Long:  `toggles between starting and stopping voice recording`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient(logLevel)
		ctx := context.Background()

		response, err := client.Toggle(ctx)
		if err != nil {
			fmt.Fprintf(os.Stderr, "cannot connect to daemon. is it running?\n")
			fmt.Fprintf(os.Stderr, "try starting the daemon with: dictator daemon\n")
			os.Exit(1)
		}

		if response.Success {
			fmt.Fprintf(os.Stderr, "toggled daemon")
		} else {
			fmt.Fprintf(os.Stderr, "toggle command failed: %s\n", response.Error)
			os.Exit(1)
		}
	},
}

var cancelCmd = &cobra.Command{
	Use:   "cancel",
	Short: "cancel current operation",
	Long:  `cancels any current recording or transcription operation`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient(logLevel)
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

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "get daemon status",
	Long:  `shows the current status of the dictator daemon`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient(logLevel)
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

var versionCmd = &cobra.Command{
	Use:   "version",
	Short: "print the version number",
	Long:  `prints the version number of the dictator daemon`,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println(version)
	},
}

var transcriptCmd = &cobra.Command{
	Use:   "transcript",
	Short: "manage transcript history",
	Long:  `commands to view and manage stored transcript history`,
}

var transcriptListCmd = &cobra.Command{
	Use:   "list",
	Short: "list all transcripts as JSON",
	Long:  `outputs all stored transcripts as JSON, ordered by timestamp (newest first)`,
	Run: func(cmd *cobra.Command, args []string) {
		db, err := storage.NewDB()
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to open database: %v\n", err)
			os.Exit(1)
		}
		defer db.Close()

		transcripts, err := db.GetAllTranscripts()
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to get transcripts: %v\n", err)
			os.Exit(1)
		}

		jsonData, err := json.MarshalIndent(transcripts, "", "  ")
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to marshal JSON: %v\n", err)
			os.Exit(1)
		}

		fmt.Println(string(jsonData))
	},
}

var transcriptLastCmd = &cobra.Command{
	Use:   "last",
	Short: "get the most recent transcript",
	Long:  `prints the text of the most recent transcript, or copies it to clipboard with --clip`,
	Run: func(cmd *cobra.Command, args []string) {
		clipFlag, _ := cmd.Flags().GetBool("clip")

		db, err := storage.NewDB()
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to open database: %v\n", err)
			os.Exit(1)
		}
		defer db.Close()

		transcript, err := db.GetLastTranscript()
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to get last transcript: %v\n", err)
			os.Exit(1)
		}

		if transcript == nil {
			fmt.Fprintf(os.Stderr, "no transcripts found\n")
			os.Exit(1)
		}

		if clipFlag {
			// Check if xclip is available
			xclipTyper := typing.XclipTyper{}
			if !xclipTyper.IsAvailable() {
				fmt.Fprintf(os.Stderr, "xclip not available - cannot copy to clipboard\n")
				os.Exit(1)
			}

			// Use xclip to copy to clipboard
			xclipTyper = typing.XclipTyper{
				Config: utils.AppConfig{},
			}

			ctx := context.Background()
			if err := xclipTyper.TypeText(ctx, transcript.Text); err != nil {
				fmt.Fprintf(os.Stderr, "failed to copy to clipboard: %v\n", err)
				os.Exit(1)
			}
			fmt.Println("Text copied to clipboard")
		} else {
			fmt.Print(transcript.Text)
		}
	},
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
	if err := utils.InitConfigFile(); err != nil {
		cobra.CheckErr(err)
	}
	if err := viper.ReadInConfig(); err != nil {
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

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default is $HOME/.config/dictator/utils.json)")
	rootCmd.PersistentFlags().StringVar(&logLevel, "log-level", "INFO", "log level (DEBUG, INFO, WARN, ERROR)")

	rootCmd.AddCommand(createDaemonCmd(config))
	rootCmd.AddCommand(startCmd)
	rootCmd.AddCommand(stopCmd)
	rootCmd.AddCommand(toggleCmd)
	rootCmd.AddCommand(cancelCmd)
	rootCmd.AddCommand(statusCmd)
	rootCmd.AddCommand(versionCmd)

	transcriptCmd.AddCommand(transcriptListCmd)
	transcriptCmd.AddCommand(transcriptLastCmd)
	transcriptLastCmd.Flags().Bool("clip", false, "copy transcript text to clipboard instead of printing")
	rootCmd.AddCommand(transcriptCmd)
}

func main() {
	utils.SetupLogger(logLevel)
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
