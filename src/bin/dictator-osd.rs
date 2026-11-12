use std::path::PathBuf;

use anyhow::{Result, bail};
use dictator::gui::osd::{OsdOptions, run};

const USAGE: &str = "usage: dictator-osd [--socket PATH] [--theme dark|light]";

/// `dictator-osd [--socket PATH] [--theme dark|light]`: the dictation island,
/// fed by the daemon's OSD socket. Rune reads its own flags (`--snap`,
/// `--scene`, ...) from the same command line.
fn main() -> Result<()> {
    let mut options = OsdOptions {
        socket: dictator::visual::default_socket_path(),
        dark: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => match args.next() {
                Some(path) => options.socket = PathBuf::from(path),
                None => bail!("--socket needs a path\n{USAGE}"),
            },
            "--theme" => match args.next().as_deref() {
                Some("dark") => options.dark = true,
                Some("light") => options.dark = false,
                _ => bail!("--theme takes dark or light\n{USAGE}"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {}
        }
    }
    run(options)
}
