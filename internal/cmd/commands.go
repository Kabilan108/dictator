package cmd

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/kabilan108/dictator/internal/logger"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
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
		Log := logger.NewLogger(logger.DEBUG, "daemon")
		Log.D("daemon called")
	},
}

var startCmd = &cobra.Command{
	Use:   "start",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := logger.NewLogger(logger.DEBUG, "start")
		Log.D("start called")
	},
}

var stopCmd = &cobra.Command{
	Use:   "stop",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := logger.NewLogger(logger.DEBUG, "stop")
		Log.D("stop called")
	},
}

var toggleCmd = &cobra.Command{
	Use:   "toggle",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := logger.NewLogger(logger.DEBUG, "toggle")
		Log.D("toggle called")
	},
}

var cancelCmd = &cobra.Command{
	Use:   "cancel",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := logger.NewLogger(logger.DEBUG, "cancel")
		Log.D("cancel called")
	},
}

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "",
	Long:  ``,
	Run: func(cmd *cobra.Command, args []string) {
		Log := logger.NewLogger(logger.DEBUG, "status")
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
