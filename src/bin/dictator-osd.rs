use std::path::PathBuf;

use anyhow::{Result, bail};
use dictator::gui::osd::{OsdOptions, Placement, run};

const USAGE: &str =
    "usage: dictator-osd [--socket PATH] [--theme dark|light] [--placement floating|bottom|top]";

const HELP: &str = "\
The dictation island: a click-through overlay that shows the dictator daemon's
recording, transcription, paste and errors.

Options:
  --socket PATH         the daemon's OSD socket
                        (default: $XDG_RUNTIME_DIR/dictator/osd.sock)
  --theme dark|light    override the system's light or dark appearance
  --placement PLACE     floating (default), a capsule above the bottom edge;
                        bottom or top, attached to that edge
  -h, --help            show this help";

/// Rune reads its own flags (`--snap`, `--scene`, ...) from the same command
/// line, so unknown arguments are left to it.
fn main() -> Result<()> {
    let mut options = OsdOptions {
        socket: dictator::visual::default_socket_path(),
        dark: None,
        placement: Placement::default(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => match args.next() {
                Some(path) => options.socket = PathBuf::from(path),
                None => bail!("--socket needs a path\n{USAGE}"),
            },
            "--theme" => match args.next().as_deref() {
                Some("dark") => options.dark = Some(true),
                Some("light") => options.dark = Some(false),
                _ => bail!("--theme takes dark or light\n{USAGE}"),
            },
            "--placement" => match args.next().as_deref().and_then(Placement::parse) {
                Some(placement) => options.placement = placement,
                None => bail!("--placement takes floating, bottom or top\n{USAGE}"),
            },
            "-h" | "--help" => {
                println!("{USAGE}\n\n{HELP}");
                return Ok(());
            }
            _ => {}
        }
    }
    run(options)
}
