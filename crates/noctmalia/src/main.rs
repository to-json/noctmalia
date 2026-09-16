//! noctmalia: a Noctalia-native front end for the Thunderbird it runs.
//!
//! Thunderbird's bridge extension reaches us through a native-messaging shim that connects to a
//! unix socket, so this process is the listener. By default it also spawns the Thunderbird on the
//! other end and takes it down again when the window closes (`crate::thunderbird`);
//! `--backend external` only listens, for a stand-in or a Thunderbird run by hand.

use noctalia_iced::{chrome, theme};
use noctmalia::app::{self, App, Backend};
use noctmalia::thunderbird::{Paths, Supervisor};
use noctmalia::{font, palette};
use noctmalia_bridge::Bridge;
use std::path::PathBuf;
use std::process::ExitCode;

const APP_ID: &str = "dev.noctalia.Noctmalia";

pub const USAGE: &str = "\
usage: noctmalia [--backend managed|external] [--dev] [--socket PATH]
       noctmalia --install-palette-template

  --backend managed   run our own headless Thunderbird (the default): fetched once
                      into $XDG_DATA_HOME/noctmalia, started with the window,
                      stopped with it. NOCTMALIA_THUNDERBIRD=/path/to/thunderbird
                      uses a build you supply instead.
  --backend external  only listen; something else attaches (tools/fake-bridge.py)
  --dev               turn on the bridge's dev methods and the control socket's
                      raw `call`, for seeding and smoke

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
    let mut managed = true;
    let mut dev = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dev" => dev = true,
            "--backend" => match arguments.next().as_deref() {
                Some("managed") => managed = true,
                Some("external") => managed = false,
                other => {
                    eprintln!("noctmalia: --backend takes managed or external, not {other:?}\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
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

    let backend = if managed {
        let Some(paths) = Paths::from_environment(bridge.path().to_path_buf()) else {
            eprintln!("noctmalia: no HOME; cannot decide where Thunderbird lives");
            return ExitCode::FAILURE;
        };
        Backend::Managed(Supervisor::spawn(paths, bridge.clone(), dev))
    } else {
        Backend::External
    };
    let supervisor = match &backend {
        Backend::Managed(supervisor) => Some(supervisor.clone()),
        Backend::External => None,
    };

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
    let application = iced::application(move || App::new(bridge.clone(), backend.clone(), dev), App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .style(|_, _| iced::theme::Style {
            background_color: chrome::background(),
            text_color: theme::palette().on_surface,
        })
        .subscription(App::subscription)
        .font(theme::ICON_FONT_BYTES)
        .window(window)
        .default_font(font::ui());
    let result = application.run();

    // The window is gone; so goes the Thunderbird behind it, through the front door.
    if let Some(supervisor) = supervisor {
        eprintln!("noctmalia: stopping Thunderbird");
        supervisor.stop();
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noctmalia: {error}");
            ExitCode::FAILURE
        }
    }
}
