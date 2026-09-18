/*
Copyright © 2025 kabilan108 tonykabilanokeke@gmail.com
*/

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use dictator::ipc::{self, Client};
use dictator::utils::{self, exit_if_error};
use dictator::{daemon, storage};

const VERSION: &str = match option_env!("DICTATOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// dictator is a voice typing daemon for linux that enables voice typing.
///
/// start the daemon with 'dictator daemon' then use commands like 'start', 'stop',
/// 'toggle', 'cancel', and 'status' to control voice recording and transcription.
#[derive(Parser)]
#[command(
    name = "dictator",
    about = "whisper typing daemon for linux",
    long_about,
    disable_version_flag = true
)]
struct Cli {
    /// log level (DEBUG, INFO, WARN, ERROR)
    #[arg(long, global = true, default_value = "INFO")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// run the dictator daemon
    ///
    /// starts the dictator daemon in the foreground, listening for voice commands via ipc
    Daemon,
    /// start voice recording
    ///
    /// tells the daemon to start recording voice input
    Start,
    /// stop voice recording and transcribe
    ///
    /// tells the daemon to stop recording and start transcription
    Stop,
    /// toggle voice recording
    ///
    /// toggles between starting and stopping voice recording
    Toggle,
    /// cancel current operation
    ///
    /// cancels any current recording or transcription operation
    Cancel,
    /// get daemon status
    ///
    /// shows the current status of the dictator daemon
    Status,
    /// print the version number
    ///
    /// prints the version number of the dictator daemon
    Version,
    /// initialize the dictator config
    ///
    /// initializes the dictator config with default values
    Init,
    /// list recent transcripts
    ///
    /// lists out the N most recent transcripts, where N is set based on the -n flag. default value is 10.
    Transcripts {
        /// number of recent transcripts to list (set to -1 for all)
        #[arg(
            short = 'n',
            long = "num",
            default_value_t = 10,
            allow_hyphen_values = true
        )]
        num: i64,
        /// print only the text of the transcripts
        #[arg(short = 't', long = "text")]
        text: bool,
    },
    /// Generate the autocompletion script for the specified shell
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
}

async fn run_command(action: &str, success_msg: &str) {
    let client = Client::new();
    let response = exit_if_error(
        client
            .send_command(action, &[])
            .await
            .map_err(daemon::not_running),
        1,
    );

    if response.success {
        println!("{success_msg}");
    } else {
        eprintln!("{action} command failed: {}", response.error);
        std::process::exit(1);
    }
}

async fn run_status() {
    let client = Client::new();
    let response = exit_if_error(client.status().await.map_err(daemon::not_running), 1);

    if response.success {
        let get = |key: &str| response.data.get(key).cloned().unwrap_or_default();
        println!("daemon status:");
        println!("  state:  {}", get(ipc::DATA_KEY_STATE));
        println!("  uptime: {}", get(ipc::DATA_KEY_UPTIME));
        if let Some(duration) = response.data.get(ipc::DATA_KEY_RECORDING_DURATION) {
            println!("  recording duration: {duration}");
        }
        if let Some(last_error) = response.data.get(ipc::DATA_KEY_LAST_ERROR) {
            println!("  last error: {last_error}");
        }
    } else {
        eprintln!("status command failed: {}", response.error);
        std::process::exit(1);
    }
}

fn run_init() {
    let config_dir = &*utils::CONFIG_DIR;
    if let Err(err) = std::fs::create_dir_all(config_dir) {
        eprintln!("failed to create config dir: {err}");
        std::process::exit(1);
    }

    let config_path = config_dir.join("config.json");
    match std::fs::metadata(&config_path) {
        Ok(_) => {
            eprintln!("Config already exists at {}", config_path.display());
            return;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            eprintln!("failed to check config: {err}");
            std::process::exit(1);
        }
    }

    let cfg = utils::default_config();
    let data = match serde_json::to_string_pretty(&cfg) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("failed to serialize default config: {err}");
            std::process::exit(1);
        }
    };

    let write = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config_path)
        .and_then(|mut f| f.write_all(data.as_bytes()));
    if let Err(err) = write {
        eprintln!("failed to write config file: {err}");
        std::process::exit(1);
    }

    eprintln!("Config written to {}", config_path.display());
    eprintln!("Update api.providers.openai.key with your API key, then run 'dictator daemon'.");
}

fn run_transcripts(num: i64, text_only: bool) {
    if num <= 0 && num != -1 {
        eprintln!("invalid value for -n: must be > 0 or -1");
        std::process::exit(1);
    }

    let db = match storage::Db::new() {
        Ok(db) => db,
        Err(err) => {
            eprintln!("failed to open database: {err:#}");
            std::process::exit(1);
        }
    };

    let transcripts = match db.get_transcripts(num) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("failed to get transcripts: {err:#}");
            std::process::exit(1);
        }
    };

    if text_only {
        for t in &transcripts {
            println!("{}", t.text);
        }
    } else {
        match serde_json::to_string_pretty(&transcripts) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("failed to marshal JSON: {err}");
                std::process::exit(1);
            }
        }
    }
}

async fn run_daemon() {
    exit_if_error(utils::ensure_directories(), 1);
    let cfg = exit_if_error(utils::get_config(), 1);
    let d = exit_if_error(daemon::Daemon::new(cfg).await, 1);
    exit_if_error(d.run().await, 1);
}

fn main() {
    let cli = Cli::parse();
    utils::setup_logger(&cli.log_level);

    let daemon_runtime = matches!(&cli.command, Commands::Daemon);
    match cli.command {
        Commands::Version => println!("{VERSION}"),
        Commands::Init => run_init(),
        Commands::Transcripts { num, text } => run_transcripts(num, text),
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "dictator", &mut std::io::stdout());
        }
        command => run_async(command, daemon_runtime),
    }
}

fn run_async(command: Commands, daemon_runtime: bool) {
    let runtime = if daemon_runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    };
    let runtime = exit_if_error(runtime.map_err(anyhow::Error::from), 1);

    runtime.block_on(async move {
        match command {
            Commands::Daemon => run_daemon().await,
            Commands::Start => run_command(ipc::ACTION_START, "Recording started").await,
            Commands::Stop => {
                run_command(ipc::ACTION_STOP, "Recording stopped, transcribing").await
            }
            Commands::Toggle => run_command(ipc::ACTION_TOGGLE, "toggled daemon").await,
            Commands::Cancel => run_command(ipc::ACTION_CANCEL, "operation canceled").await,
            Commands::Status => run_status().await,
            Commands::Version
            | Commands::Init
            | Commands::Transcripts { .. }
            | Commands::Completion { .. } => unreachable!("sync command routed to async runtime"),
        }
    });
}
