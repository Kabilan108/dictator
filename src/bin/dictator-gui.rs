use anyhow::Result;
use dictator::gui::{GuiOptions, run};

/// `dictator-gui [--demo]`. `--demo` uses isolated synthetic recordings,
/// statistics and microphones. Rune reads its own flags (`--snap`, `--scene`,
/// `--theme`, ...) from the same command line and hands back the rest.
fn main() -> Result<()> {
    let demo = rune::extra_args().iter().any(|arg| arg == "--demo");
    run(GuiOptions { demo })
}
