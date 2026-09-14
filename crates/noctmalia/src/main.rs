//! noctmalia: a Noctalia-native front end for Thunderbird running headless as a daemon.
//!
//! Thunderbird's bridge extension reaches us through a native-messaging shim that connects to a
//! unix socket, so this process is the listener. With Thunderbird in a container, bind-mount the
//! socket's directory into it (`docs/findings.md` §9).

use iced::Size;
use noctalia_iced::{chrome, theme};
use noctmalia::app::Rolodex;
use noctmalia::{font, palette};
use noctmalia_bridge::Bridge;
use std::path::PathBuf;
use std::process::ExitCode;

const APP_ID: &str = "dev.noctalia.Noctmalia";

pub const USAGE: &str = "\
usage: noctmalia [--socket PATH]

  --socket PATH  where to listen for Thunderbird's shim
                 (default: $NOCTMALIA_BRIDGE_SOCKET, else
                 $XDG_RUNTIME_DIR/noctmalia/bridge.sock)
  --help         this
";

/// `$XDG_RUNTIME_DIR` is per-user and cleaned on logout, which is what a socket wants. Falling back
/// to `/tmp` keeps the app usable where it is unset, at the cost of a path anyone can guess.
fn default_socket() -> PathBuf {
    if let Some(path) = std::env::var_os("NOCTMALIA_BRIDGE_SOCKET") {
        return PathBuf::from(path);
    }
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    run.join("noctmalia").join("bridge.sock")
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let mut socket = default_socket();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--socket" => match arguments.next() {
                Some(path) => socket = PathBuf::from(path),
                None => {
                    eprintln!("noctmalia: --socket needs a path\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("noctmalia: unexpected argument: {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let bridge = match Bridge::spawn(&socket) {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("noctmalia: cannot listen on {}: {error}", socket.display());
            return ExitCode::FAILURE;
        }
    };
    eprintln!("noctmalia: listening on {}", bridge.path().display());

    // Correct iced's generic families first, so anything that never names one — iced's own widgets
    // included — lands on the desktop's fonts; then name ours explicitly for the chrome and content.
    font::adopt_system_families();
    theme::set_font(font::ui());

    // Follow the shell's colours from the first frame; the app keeps them current after that.
    match palette::load() {
        Some(found) => {
            eprintln!("noctmalia: following the Noctalia palette");
            theme::set_palette(found);
        }
        None => eprintln!("noctmalia: no Noctalia palette found, using the built-in one"),
    }

    let window = chrome::settings(Size::new(1100.0, 720.0), Size::new(760.0, 480.0), APP_ID);
    let application = iced::application(move || Rolodex::new(bridge.clone()), Rolodex::update, Rolodex::view)
        .title(Rolodex::title)
        .theme(Rolodex::theme)
        .style(|_, _| iced::theme::Style {
            background_color: chrome::background(),
            text_color: theme::palette().on_surface,
        })
        .subscription(Rolodex::subscription)
        .font(theme::ICON_FONT_BYTES)
        .window(window)
        .default_font(font::ui());
    let result = application.run();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noctmalia: {error}");
            ExitCode::FAILURE
        }
    }
}
