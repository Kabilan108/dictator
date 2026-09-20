use anyhow::Result;
use clap::Parser;
use dictator::gui::{GuiOptions, run};

#[derive(Debug, Parser)]
#[command(about = "Dictator desktop history and recording controls")]
struct Args {
    /// Use isolated synthetic recordings, statistics and microphones.
    #[arg(long)]
    demo: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(GuiOptions { demo: args.demo })
}
