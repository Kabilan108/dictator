use anyhow::Result;
use clap::Parser;
use dictator::gui::{GuiOptions, run};

#[derive(Debug, Parser)]
#[command(about = "Dictator desktop history and recording controls")]
struct Args {
    /// Use isolated synthetic recordings, statistics and microphones.
    #[arg(long)]
    demo: bool,

    /// Start in the system tray without opening the main history window.
    #[arg(long)]
    tray: bool,

    /// Run without a status notifier. Useful for isolated UI checks.
    #[arg(long, conflicts_with = "tray")]
    no_tray: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(GuiOptions {
        demo: args.demo,
        show_main: !args.tray,
        tray: !args.no_tray,
    })
}
