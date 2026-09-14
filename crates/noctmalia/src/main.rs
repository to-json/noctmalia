//! noctmalia: a Noctalia-native front end for Thunderbird running headless as a daemon.
//!
//! Thunderbird's bridge extension reaches us through a native-messaging shim that connects to a
//! unix socket, so this process is the listener. With Thunderbird in a container, bind-mount the
//! socket's directory into it (`docs/findings.md` §9).

use noctalia_iced::{chrome, theme};
use noctmalia::app::{self, Rolodex};
use noctmalia::{font, palette};
use noctmalia_bridge::Bridge;
use std::path::PathBuf;
use std::process::ExitCode;

const APP_ID: &str = "dev.noctalia.Noctmalia";

pub const USAGE: &str = "\
usage: noctmalia [--socket PATH]
       noctmalia --install-palette-template

  --socket PATH  where to listen for Thunderbird's shim
                 (default: $NOCTMALIA_BRIDGE_SOCKET, else
                 $XDG_RUNTIME_DIR/noctmalia/bridge.sock)

  --install-palette-template
                 ask Noctalia to render its palette where this app can read it,
                 so the window follows your theme. Once per machine; see the
                 `palette` module for what it writes and how to undo it.

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

/// Registers our template with Noctalia. Split out of `main` because it is a one-shot: it writes
/// two files, says what it did, and never opens a window.
fn install_palette_template() -> ExitCode {
    match palette::install() {
        Ok(done) => {
            println!("wrote {}", done.template.display());
            println!("wrote {}", done.registration.display());
            if done.applied {
                println!("Noctalia rendered {}", done.rendered.display());
            } else {
                println!(
                    "Noctalia is not running, so nothing is rendered yet — it will write\n\
                     {} on the next theme change.",
                    done.rendered.display()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("noctmalia: cannot install the palette template: {error}");
            ExitCode::FAILURE
        }
    }
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
            "--install-palette-template" => return install_palette_template(),
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
        None => eprintln!(
            "noctmalia: no Noctalia palette rendered yet, using the built-in one\n\
             noctmalia: run `noctmalia --install-palette-template` to follow your theme"
        ),
    }

    let window = chrome::settings(app::WINDOW, app::WINDOW_MIN, APP_ID);
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
