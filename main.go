/*
Copyright © 2025 kabilan108 tonykabilanokeke@gmail.com
*/

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"

	"github.com/kabilan108/dictator/internal/daemon"
	"github.com/kabilan108/dictator/internal/ipc"
	"github.com/kabilan108/dictator/internal/storage"
	"github.com/kabilan108/dictator/internal/utils"
)

var (
	logLevel string
	version  = "dev"
)

func runCommand(action string, successMsg string) {
	client := ipc.NewClient()
	response, err := client.SendCommand(context.Background(), action)
	utils.ExitIfError(daemon.NotRunning(err), 1)

	if response.Success {
		fmt.Println(successMsg)
	} else {
		fmt.Fprintf(os.Stderr, "%s command failed: %s\n", action, response.Error)
		os.Exit(1)
	}
}

var rootCmd = &cobra.Command{
	Use:   "dictator",
	Short: "whisper typing daemon for linux",
	Long: `dictator is a voice typing daemon for linux that enables voice typing.

start the daemon with 'dictator daemon' then use commands like 'start', 'stop',
'toggle', 'cancel', and 'status' to control voice recording and transcription.`,
	PersistentPreRun: func(cmd *cobra.Command, args []string) {
		utils.SetupLogger(logLevel)
	},
}

var daemonCmd = &cobra.Command{
	Use:   "daemon",
	Short: "run the dictator daemon",
	Long:  `starts the dictator daemon in the foreground, listening for voice commands via ipc`,
	Run: func(cmd *cobra.Command, args []string) {
		utils.ExitIfError(utils.EnsureDirectories(), 1)

		c, err := utils.GetConfig()
		utils.ExitIfError(err, 1)

		d, err := daemon.NewDaemon(c)
		utils.ExitIfError(err, 1)

		err = d.Run()
		utils.ExitIfError(err, 1)
	},
}

var startCmd = &cobra.Command{
	Use:   "start",
	Short: "start voice recording",
	Long:  `tells the daemon to start recording voice input`,
	Run: func(cmd *cobra.Command, args []string) {
		runCommand(ipc.ActionStart, "Recording started")
	},
}

var stopCmd = &cobra.Command{
	Use:   "stop",
	Short: "stop voice recording and transcribe",
	Long:  `tells the daemon to stop recording and start transcription`,
	Run: func(cmd *cobra.Command, args []string) {
		runCommand(ipc.ActionStop, "Recording stopped, transcribing")
	},
}

var toggleCmd = &cobra.Command{
	Use:   "toggle",
	Short: "toggle voice recording",
	Long:  `toggles between starting and stopping voice recording`,
	Run: func(cmd *cobra.Command, args []string) {
		runCommand(ipc.ActionToggle, "toggled daemon")
	},
}

var cancelCmd = &cobra.Command{
	Use:   "cancel",
	Short: "cancel current operation",
	Long:  `cancels any current recording or transcription operation`,
	Run: func(cmd *cobra.Command, args []string) {
		runCommand(ipc.ActionCancel, "operation canceled")
	},
}

var streamCmd = &cobra.Command{
	Use:   "stream",
	Short: "start streaming transcription",
	Long:  `starts real-time streaming transcription (alternative to start/stop)`,
	Run: func(cmd *cobra.Command, args []string) {
		runCommand(ipc.ActionStream, "streaming started")
	},
}

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "get daemon status",
	Long:  `shows the current status of the dictator daemon`,
	Run: func(cmd *cobra.Command, args []string) {
		client := ipc.NewClient()
		ctx := context.Background()

		response, err := client.Status(ctx)
		utils.ExitIfError(daemon.NotRunning(err), 1)

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
			fmt.Fprintf(os.Stderr, "status command failed: %s\n", response.Error)
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

var initCmd = &cobra.Command{
	Use:   "init",
	Short: "initialize the dictator config",
	Long:  `initializes the dictator config with default values`,
	Run: func(cmd *cobra.Command, args []string) {
		configDir := utils.CONFIG_DIR
		if err := os.MkdirAll(configDir, 0o755); err != nil {
			fmt.Fprintf(os.Stderr, "failed to create config dir: %v\n", err)
			os.Exit(1)
		}

		configPath := filepath.Join(configDir, "config.json")
		if _, err := os.Stat(configPath); err == nil {
			fmt.Fprintf(os.Stderr, "Config already exists at %s\n", configPath)
			return
		} else if !os.IsNotExist(err) {
			fmt.Fprintf(os.Stderr, "failed to check config: %v\n", err)
			os.Exit(1)
		}

		cfg := utils.DefaultConfig()
		data, err := json.MarshalIndent(cfg, "", "  ")
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to serialize default config: %v\n", err)
			os.Exit(1)
		}

		if err := os.WriteFile(configPath, data, 0o600); err != nil {
			fmt.Fprintf(os.Stderr, "failed to write config file: %v\n", err)
			os.Exit(1)
		}

		fmt.Fprintf(os.Stderr, "Config written to %s\n", configPath)
		fmt.Fprintf(os.Stderr, "Update api.providers.openai.key with your API key, then run 'dictator daemon'.\n")
	},
}

var transcriptsCmd = &cobra.Command{
	Use:   "transcripts",
	Short: "list recent transcripts",
	Long:  `lists out the N most recent transcripts, where N is set based on the -n flag. default value is 10.`,
	Run: func(cmd *cobra.Command, args []string) {
		n, _ := cmd.Flags().GetInt("num")
		textOnly, _ := cmd.Flags().GetBool("text")

		if n <= 0 && n != -1 {
			fmt.Fprintf(os.Stderr, "invalid value for -n: must be > 0 or -1\n")
			os.Exit(1)
		}

		db, err := storage.NewDB()
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to open database: %v\n", err)
			os.Exit(1)
		}
		defer db.Close()

		var transcripts []storage.Transcript

		if n == -1 {
			transcripts, err = db.GetTranscripts(-1)
		} else {
			transcripts, err = db.GetTranscripts(n)
		}

		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to get transcripts: %v\n", err)
			os.Exit(1)
		}

		if textOnly {
			for _, t := range transcripts {
				fmt.Println(t.Text)
			}
		} else {
			jsonData, err := json.MarshalIndent(transcripts, "", "  ")
			if err != nil {
				fmt.Fprintf(os.Stderr, "failed to marshal JSON: %v\n", err)
				os.Exit(1)
			}
			fmt.Println(string(jsonData))
		}
	},
}

func init() {
	rootCmd.PersistentFlags().StringVar(&logLevel, "log-level", "INFO", "log level (DEBUG, INFO, WARN, ERROR)")
	rootCmd.AddCommand(daemonCmd)
	rootCmd.AddCommand(startCmd)
	rootCmd.AddCommand(stopCmd)
	rootCmd.AddCommand(toggleCmd)
	rootCmd.AddCommand(cancelCmd)
	rootCmd.AddCommand(statusCmd)
	rootCmd.AddCommand(streamCmd)
	rootCmd.AddCommand(versionCmd)
	rootCmd.AddCommand(initCmd)

	transcriptsCmd.Flags().IntP("num", "n", 10, "number of recent transcripts to list (set to -1 for all)")
	transcriptsCmd.Flags().BoolP("text", "t", false, "print only the text of the transcripts")
	rootCmd.AddCommand(transcriptsCmd)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
