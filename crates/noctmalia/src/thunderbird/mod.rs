//! The Thunderbird this program runs.
//!
//! noctmalia starts and stops like a normal program, and the Thunderbird behind it is an
//! implementation detail: a pinned Mozilla build fetched once, a private profile, a headless
//! process spawned into a transient systemd user scope when the window opens and asked to leave
//! when it closes. The user never sees any of it. `docs/one-program-plan.md` is the plan this
//! is built from; `spike/native-probe/run.sh` is the same sequence done by hand.
//!
//! Three parts. [`fetch`] gets the build. [`provision`] writes what a start needs into the
//! profile and the install: prefs, the bridge extension, the native-messaging shim and the
//! manifest that names it. [`Supervisor`] spawns, watches, restarts once, and stops — through
//! `bridge.quit`, because SIGTERM is an instant death for Gecko that leaves the profile locked
//! and its databases mid-write.

pub mod fetch;
pub mod provision;

use crate::error::Error;
use noctmalia_bridge::Bridge;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// The Thunderbird release this program is built against and tested with.
pub const VERSION: &str = "155.0.1";

/// The transient scope Thunderbird runs in, so the process tree is one unit.
pub const SCOPE: &str = "noctmalia-thunderbird";

/// The extension id, which is also the XPI's file name and the only id the native-messaging
/// manifest admits.
pub const EXTENSION_ID: &str = "bridge@noctmalia";

/// How long a clean quit is given before the scope is stopped from outside.
const QUIT_GRACE: Duration = Duration::from_secs(10);
/// How long a stop from outside is given before the process group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// A second unexpected exit inside this window is a failure rather than a restart.
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Where everything lives, from the XDG base directories.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$XDG_DATA_HOME/noctmalia/thunderbird/<version>`, or wherever `NOCTMALIA_THUNDERBIRD`
    /// points: the directory holding the `thunderbird` launcher.
    pub install: PathBuf,
    /// `$XDG_DATA_HOME/noctmalia/profile`.
    pub profile: PathBuf,
    /// `$XDG_DATA_HOME/noctmalia/nm-shim.py`, the native-messaging host.
    pub shim: PathBuf,
    /// `~/.mozilla/native-messaging-hosts/noctmalia.bridge.json`. User-global by Mozilla's
    /// design; it admits only [`EXTENSION_ID`], so a Thunderbird or Firefox of the user's own
    /// sees it and ignores it.
    pub manifest: PathBuf,
    /// `$XDG_CACHE_HOME/noctmalia`, where the tarball lands.
    pub cache: PathBuf,
    /// `$XDG_STATE_HOME/noctmalia/thunderbird.log`.
    pub log: PathBuf,
    /// The socket Thunderbird's shim connects to.
    pub socket: PathBuf,
}

impl Paths {
    pub fn from_environment(socket: PathBuf) -> Option<Paths> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let xdg = |name: &str, fallback: &str| {
            std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| home.join(fallback)).join("noctmalia")
        };
        let data = xdg("XDG_DATA_HOME", ".local/share");
        let install = match std::env::var_os("NOCTMALIA_THUNDERBIRD") {
            Some(binary) => PathBuf::from(binary).parent().map(Path::to_path_buf)?,
            None => data.join("thunderbird").join(VERSION),
        };
        Some(Paths {
            install,
            profile: data.join("profile"),
            shim: data.join("nm-shim.py"),
            manifest: home.join(".mozilla/native-messaging-hosts/noctmalia.bridge.json"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            log: xdg("XDG_STATE_HOME", ".local/state").join("thunderbird.log"),
            socket,
        })
    }

    pub fn binary(&self) -> PathBuf {
        self.install.join("thunderbird")
    }
}

/// What the supervisor is doing, as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The pinned build is being downloaded, for the first and only time.
    Fetching { received: u64, total: u64 },
    /// Provisioned and spawned; the hello is on its way.
    Starting,
    /// Thunderbird exited on its own and is being started again.
    Restarting,
    /// It exited twice in a minute, or could not be fetched or spawned at all. The log says why.
    Failed { reason: String, log: PathBuf },
}

/// A snapshot for the control socket's `status`.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub pid: Option<u32>,
    /// Whether the process is in the systemd scope, or a plain process group.
    pub scoped: bool,
    pub started: Option<Instant>,
    pub restarts: u32,
    pub state: String,
}

impl Status {
    pub fn json(&self) -> serde_json::Value {
        json!({
            "pid": self.pid,
            "scope": self.scoped.then_some(format!("{SCOPE}.scope")),
            "uptime_seconds": self.started.map(|at| at.elapsed().as_secs()),
            "restarts": self.restarts,
            "state": self.state,
        })
    }
}

enum Order {
    Stop(mpsc::Sender<()>),
}

/// The thread that owns Thunderbird for the life of the window. Cheap to clone; every clone
/// talks to the same thread.
#[derive(Clone)]
pub struct Supervisor {
    orders: mpsc::Sender<Order>,
    reports: broadcast::Sender<Report>,
    status: Arc<Mutex<Status>>,
}

/// How Thunderbird is launched: the real thing, or a stand-in for a test.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The environment, from nothing. The nix devshell's `LD_LIBRARY_PATH` reaches every child
    /// otherwise, and a Mozilla build linked against system GTK does not survive nix's Mesa
    /// underneath it.
    pub env: Vec<(String, String)>,
}

impl Launch {
    /// Headless, on the profile, with the socket in its environment.
    pub fn headless(paths: &Paths) -> Launch {
        let mut env = allowlisted_environment();
        env.push(("NOCTMALIA_BRIDGE_SOCKET".into(), paths.socket.display().to_string()));
        env.push(("MOZ_CRASHREPORTER_DISABLE".into(), "1".into()));
        env.push(("NO_AT_BRIDGE".into(), "1".into()));
        let args = vec![
            "--headless".to_string(),
            "--profile".to_string(),
            paths.profile.display().to_string(),
            "--no-remote".to_string(),
        ];
        Launch { program: paths.binary(), args, env }
    }
}

/// What a child may inherit: enough to find the home directory, the session bus and the socket
/// directory, and nothing that names a library.
fn allowlisted_environment() -> Vec<(String, String)> {
    ["HOME", "PATH", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "LANG", "LC_ALL", "TZ"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.to_string(), value)))
        .collect()
}

impl Supervisor {
    /// Fetches, provisions, spawns, and watches, on a thread of its own that lives until
    /// [`stop`](Supervisor::stop). `dev` turns the bridge's dev pref on in the profile.
    pub fn spawn(paths: Paths, bridge: Bridge, dev: bool) -> Supervisor {
        let (orders, inbox) = mpsc::channel();
        let (reports, _) = broadcast::channel(16);
        let status = Arc::new(Mutex::new(Status { state: "starting".into(), ..Status::default() }));
        let supervisor = Supervisor { orders, reports: reports.clone(), status: Arc::clone(&status) };
        // The thread is what `PR_SET_PDEATHSIG` is measured against: the signal fires when the
        // thread that spawned the child exits, so the spawning thread has to be this one and it
        // has to outlive every child.
        std::thread::Builder::new()
            .name("noctmalia-thunderbird".into())
            .spawn(move || run(paths, bridge, dev, inbox, reports, status))
            .expect("a supervisor thread");
        supervisor
    }

    pub fn reports(&self) -> broadcast::Receiver<Report> {
        self.reports.subscribe()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Asks Thunderbird to leave and waits until it has, one way or another. Returns when the
    /// process is gone; up to about fifteen seconds in the worst case.
    pub fn stop(&self) {
        let (done, gone) = mpsc::channel();
        if self.orders.send(Order::Stop(done)).is_ok() {
            let _ = gone.recv_timeout(QUIT_GRACE + STOP_GRACE + Duration::from_secs(2));
        }
    }
}

/// One running Thunderbird, however it was started.
struct Running {
    child: Child,
    scoped: bool,
    since: Instant,
}

fn run(
    paths: Paths,
    bridge: Bridge,
    dev: bool,
    inbox: mpsc::Receiver<Order>,
    reports: broadcast::Sender<Report>,
    status: Arc<Mutex<Status>>,
) {
    let report = |report: Report| {
        if let Ok(mut status) = status.lock() {
            status.state = match &report {
                Report::Fetching { .. } => "fetching",
                Report::Starting => "starting",
                Report::Restarting => "restarting",
                Report::Failed { .. } => "failed",
            }
            .to_string();
        }
        let _ = reports.send(report);
    };
    let failed = |reason: String| {
        eprintln!("noctmalia: thunderbird: {reason}");
        report(Report::Failed { reason, log: paths.log.clone() });
    };

    if let Err(error) = fetch::ensure(&paths, |received, total| report(Report::Fetching { received, total })) {
        failed(error.to_string());
        wait_for_stop(&inbox);
        return;
    }
    stop_stale_scope();

    let mut restarts = 0u32;
    loop {
        if let Err(error) = provision::everything(&paths, dev) {
            failed(format!("provisioning the profile: {error}"));
            wait_for_stop(&inbox);
            return;
        }
        report(if restarts == 0 { Report::Starting } else { Report::Restarting });
        let mut running = match start(Launch::headless(&paths), &paths.log) {
            Ok(running) => running,
            Err(error) => {
                failed(format!("starting {}: {error}", paths.binary().display()));
                wait_for_stop(&inbox);
                return;
            }
        };
        if let Ok(mut status) = status.lock() {
            status.pid = Some(running.child.id());
            status.scoped = running.scoped;
            status.started = Some(running.since);
            status.restarts = restarts;
            status.state = "running".into();
        }

        // Watch the child and the inbox at once: a waiter thread turns the exit into a message.
        let (exits, exited) = mpsc::channel();
        let (pid, scoped) = (running.child.id(), running.scoped);
        let waiter_pid = pid;
        std::thread::spawn(move || {
            let status = wait_pid(waiter_pid);
            let _ = exits.send(status);
        });
        enum Next {
            Exited(Option<i32>),
            Stop(mpsc::Sender<()>),
        }
        let next = loop {
            if let Ok(status) = exited.try_recv() {
                break Next::Exited(status);
            }
            match inbox.recv_timeout(Duration::from_millis(200)) {
                Ok(Order::Stop(done)) => break Next::Stop(done),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // Nobody holds the supervisor any more: the window is gone without saying so.
                Err(mpsc::RecvTimeoutError::Disconnected) => break Next::Stop(mpsc::channel().0),
            }
        };
        match next {
            Next::Stop(done) => {
                stop(&bridge, &mut running, &exited);
                if let Ok(mut status) = status.lock() {
                    status.pid = None;
                    status.state = "stopped".into();
                }
                let _ = done.send(());
                return;
            }
            Next::Exited(code) => {
                let _ = running.child.wait();
                let _ = (pid, scoped);
                let uptime = running.since.elapsed();
                eprintln!("noctmalia: thunderbird exited ({code:?}) after {:.0}s", uptime.as_secs_f32());
                if uptime < RESTART_WINDOW && restarts > 0 {
                    failed(format!("Thunderbird exited twice within a minute (last status {code:?})"));
                    wait_for_stop(&inbox);
                    return;
                }
                restarts += 1;
            }
        }
    }
}

/// Once there is nothing to supervise, the only thing left to do is answer a stop.
fn wait_for_stop(inbox: &mpsc::Receiver<Order>) {
    if let Ok(Order::Stop(done)) = inbox.recv() {
        let _ = done.send(());
    }
}

/// Blocks until `pid` exits; the exit status, or `None` for a signal.
fn wait_pid(pid: u32) -> Option<i32> {
    let mut status: libc::c_int = 0;
    // SAFETY: waitpid on a pid this process spawned; the pointer is to a live local.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
    if waited != pid as libc::pid_t {
        return None;
    }
    libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status))
}

/// Spawns `launch` under a transient systemd user scope, or in its own process group where
/// there is no user session to ask.
fn start(launch: Launch, log: &Path) -> std::io::Result<Running> {
    let since = Instant::now();
    match spawn_scoped(&launch, log) {
        Ok(child) => Ok(Running { child, scoped: true, since }),
        Err(error) => {
            eprintln!("noctmalia: no systemd user scope ({error}); running Thunderbird in a process group");
            let child = spawn_plain(&launch, log)?;
            Ok(Running { child, scoped: false, since })
        }
    }
}

fn spawn_scoped(launch: &Launch, log: &Path) -> std::io::Result<Child> {
    let mut command = Command::new("systemd-run");
    command.args(["--user", "--scope", "--collect", "--quiet", "--unit", SCOPE, "--"]);
    command.arg(&launch.program).args(&launch.args);
    // systemd-run needs the session bus to talk to the user manager; the child gets the same
    // allowlist, since a scope runs the program in the caller's environment.
    command.env_clear().envs(launch.env.iter().cloned());
    if let Ok(bus) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        command.env("DBUS_SESSION_BUS_ADDRESS", bus);
    }
    let mut child = spawn_logged(command, log)?;
    // systemd-run exits at once, non-zero, when it could not start the scope.
    std::thread::sleep(Duration::from_millis(300));
    if let Some(status) = child.try_wait()? {
        return Err(std::io::Error::other(format!("systemd-run exited with {status}")));
    }
    Ok(child)
}

fn spawn_plain(launch: &Launch, log: &Path) -> std::io::Result<Child> {
    let mut command = Command::new(&launch.program);
    command.args(&launch.args).env_clear().envs(launch.env.iter().cloned()).process_group(0);
    spawn_logged(command, log)
}

/// Spawns with `PR_SET_PDEATHSIG` and both output streams into the log, through a filter that
/// drops the GTK icon-theme noise a headless Gecko prints per icon lookup.
fn spawn_logged(mut command: Command, log: &Path) -> std::io::Result<Child> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // SAFETY: prctl is async-signal-safe and touches nothing but this process's own flags.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    let file = Arc::new(Mutex::new(file));
    for stream in [child.stdout.take().map(Stream::Out), child.stderr.take().map(Stream::Err)].into_iter().flatten() {
        let file = Arc::clone(&file);
        std::thread::spawn(move || {
            let reader: Box<dyn std::io::Read + Send> = match stream {
                Stream::Out(out) => Box::new(out),
                Stream::Err(err) => Box::new(err),
            };
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if is_noise(&line) {
                    continue;
                }
                if let Ok(mut file) = file.lock() {
                    let _ = writeln!(file, "{line}");
                }
            }
        });
    }
    Ok(child)
}

enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

/// Lines not worth keeping: the icon-theme assertion headless GTK prints three of per icon.
fn is_noise(line: &str) -> bool {
    line.trim().is_empty() || line.contains("gtk_icon_theme") || line.contains("Gtk-CRITICAL")
}

/// Stops a running Thunderbird: the front door first, then the scope, then the signal.
fn stop(bridge: &Bridge, running: &mut Running, exited: &mpsc::Receiver<Option<i32>>) {
    // Only a Thunderbird that is attached can be asked. The reply usually loses the race with the
    // exit and comes back as "not connected", which is fine: what matters is the exit itself.
    let asked = bridge.stats().connection.is_some()
        && tokio::runtime::Builder::new_current_thread().enable_all().build().is_ok_and(|runtime| {
            runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(2), bridge.call_raw("bridge.quit", json!({}))).await
                })
                .is_ok()
        });
    if asked && exited.recv_timeout(QUIT_GRACE).is_ok() {
        return;
    }
    if !asked {
        eprintln!("noctmalia: thunderbird did not take bridge.quit; stopping it from outside");
    } else {
        eprintln!("noctmalia: thunderbird did not leave within {}s; stopping it from outside", QUIT_GRACE.as_secs());
    }
    if running.scoped {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{SCOPE}.scope")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        // SAFETY: a negative pid names the process group this process created for the child.
        unsafe {
            libc::kill(-(running.child.id() as libc::pid_t), libc::SIGTERM);
        }
    }
    if exited.recv_timeout(STOP_GRACE).is_ok() {
        return;
    }
    let _ = running.child.kill();
    let _ = exited.recv_timeout(Duration::from_secs(2));
}

/// A previous noctmalia that was SIGKILLed leaves its scope behind, still holding the profile.
fn stop_stale_scope() {
    let unit = format!("{SCOPE}.scope");
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", &unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !active {
        return;
    }
    eprintln!("noctmalia: a Thunderbird from an earlier run is still up in {unit}; stopping it");
    let _ =
        Command::new("systemctl").args(["--user", "stop", &unit]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let deadline = Instant::now() + STOP_GRACE + Duration::from_secs(2);
    while Instant::now() < deadline {
        let still = Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", &unit])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !still {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Error {
        Error::Local(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> Bridge {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let which = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("noctmalia-tb-test-{}-{which}.sock", std::process::id()));
        Bridge::spawn(path).expect("a socket")
    }

    fn systemd_here() -> bool {
        Command::new("systemd-run")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    #[test]
    fn the_environment_a_child_gets_names_no_library_paths() {
        let env = allowlisted_environment();
        assert!(env.iter().all(|(name, _)| !name.starts_with("LD_") && !name.contains("GL")));
        assert!(env.iter().any(|(name, _)| name == "HOME"));
    }

    #[test]
    fn noise_is_the_icon_theme_assertion_and_blank_lines() {
        assert!(is_noise("(thunderbird:1): Gtk-CRITICAL **: gtk_icon_theme_lookup_icon: assertion failed"));
        assert!(is_noise("   "));
        assert!(!is_noise("*** You are running in headless mode."));
    }

    /// The whole life of a stand-in "Thunderbird": spawned under the scope, watched, stopped from
    /// outside when it ignores `bridge.quit` (nothing is attached to answer it), and gone.
    #[test]
    fn a_stand_in_is_spawned_under_the_scope_and_stopped_from_outside() {
        if !systemd_here() {
            eprintln!("skipping: no systemd user session");
            return;
        }
        let log = std::env::temp_dir().join(format!("noctmalia-tb-test-{}.log", std::process::id()));
        let launch = Launch { program: "/bin/sleep".into(), args: vec!["60".into()], env: allowlisted_environment() };
        let mut running = start(launch, &log).expect("sleep starts");
        assert!(running.scoped, "systemd-run took it");
        let unit = format!("{SCOPE}.scope");
        let active =
            Command::new("systemctl").args(["--user", "is-active", "--quiet", &unit]).status().unwrap().success();
        assert!(active, "the scope is up while the child runs");

        let (exits, exited) = mpsc::channel();
        let pid = running.child.id();
        std::thread::spawn(move || {
            let _ = exits.send(wait_pid(pid));
        });
        let started = Instant::now();
        stop(&bridge(), &mut running, &exited);
        assert!(started.elapsed() < QUIT_GRACE, "nothing was attached to answer bridge.quit, so no grace was spent");
        let _ = running.child.wait();
        let active =
            Command::new("systemctl").args(["--user", "is-active", "--quiet", &unit]).status().unwrap().success();
        assert!(!active, "the scope is gone");
        let _ = std::fs::remove_file(log);
    }
}
