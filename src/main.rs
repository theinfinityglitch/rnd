use rnd::*;

use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc::sync_channel, Arc, Mutex};
use std::{env, fs};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::Application;

const APP_ID: &str = "org.rnd.NotificationDaemon";
const DEFAULT_CSS: &str = include_str!("../assets/style.css");

fn main() {
    // CLI helpers: install, install-rndctl, uninstall
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--uninstall") {
        let skip_confirm = args.iter().any(|a| a == "--yes");
        if !skip_confirm {
            use std::io::{self, Write};
            eprint!("Are you sure you want to uninstall rnd and remove installed files? [y/N] ");
            io::stdout().flush().ok();
            let mut line = String::new();
            if let Ok(_) = io::stdin().read_line(&mut line) {
                let ans = line.trim().to_lowercase();
                if ans != "y" && ans != "yes" {
                    println!("Uninstall aborted.");
                    return;
                }
            } else {
                println!("Failed to read input; aborting.");
                return;
            }
        }

        if let Err(e) = do_uninstall() {
            eprintln!("Uninstall failed: {}", e);
            std::process::exit(1);
        }
        println!("Uninstalled rnd.");
        return;
    }

    if args.iter().any(|a| a == "--install-rndctl") {
        // optional --rndctl-path=/path/to/rndctl
        let path = args
            .iter()
            .find(|s| s.starts_with("--rndctl-path="))
            .and_then(|s| s.splitn(2, '=').nth(1).map(|p| PathBuf::from(p)));
        if let Err(e) = do_install_rndctl(path) {
            eprintln!("rndctl install failed: {}", e);
            std::process::exit(1);
        }
        println!("rndctl install completed.");
        return;
    }

    // Simple helper: if run with `--install` behave like install.sh but from
    // the running binary. This makes it easy for users to install without
    // running the separate script.
    if args.iter().any(|a| a == "--install") {
        let no_start = args.iter().any(|a| a == "--no-start");
        if let Err(e) = do_install(!no_start) {
            eprintln!("Install failed: {}", e);
            std::process::exit(1);
        }
        println!("Install completed.");
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("rnd=info")),
        )
        .init();

    #[cfg(unix)]
    // SAFETY: called before any other threads exist; SIG_IGN is always safe.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let config = Arc::new(config::Config::load());

    let app = Application::builder().application_id(APP_ID).build();

    let config_clone = Arc::clone(&config);
    app.connect_activate(move |app| activate(app, Arc::clone(&config_clone)));

    let exit_code: i32 = app.run().into();
    std::process::exit(exit_code);
}

fn do_install(start: bool) -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let install_bin = dirs::home_dir()
        .map(|h| h.join(".local/bin/rnd"))
        .ok_or("Could not determine home directory")?;

    fs::create_dir_all(install_bin.parent().unwrap())?;
    println!("Installing binary → {}", install_bin.display());
    // Write the running executable bytes to a temporary file and rename into
    // place. This avoids "Text file busy" errors when the destination is
    // currently executing (common when installing over the running binary).
    let exe_bytes = fs::read(&exe)?;
    let temp_path = install_bin.with_extension("part");
    fs::write(&temp_path, &exe_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_path, perms)?;
    }
    fs::rename(&temp_path, &install_bin)?;

    // systemd unit
    let systemd_dir = dirs::home_dir()
        .map(|h| h.join(".config/systemd/user"))
        .ok_or("Could not determine home directory")?;
    fs::create_dir_all(&systemd_dir)?;
    let rnd_service_src = include_str!("../contrib/rnd.service");
    let service_contents =
        rnd_service_src.replace("%h/.cargo/bin/rnd", &install_bin.to_string_lossy());
    let service_path = systemd_dir.join("rnd.service");
    fs::write(&service_path, service_contents)?;

    // D-Bus activation file
    let dbus_dir = dirs::home_dir()
        .map(|h| h.join(".local/share/dbus-1/services"))
        .ok_or("Could not determine home directory")?;
    fs::create_dir_all(&dbus_dir)?;
    let dbus_src = include_str!("../contrib/org.freedesktop.Notifications.service");
    let dbus_contents = dbus_src.replace("@@RND_BIN@@", &install_bin.to_string_lossy());
    let dbus_path = dbus_dir.join("org.freedesktop.Notifications.service");
    fs::write(&dbus_path, dbus_contents)?;

    // Reload systemd user units
    let _ = Command::new("systemctl")
        .arg("--user")
        .arg("daemon-reload")
        .status();

    // Try to install rndctl companion if a built binary exists nearby.
    // Check sibling 'rndctl' near the running exe and 'target/release/rndctl'
    // in the current working directory.
    let mut rndctl_installed = false;
    let cand1 = exe.parent().map(|p| p.join("rndctl"));
    let cand2 = std::env::current_dir()
        .ok()
        .map(|d| d.join("target/release/rndctl"));
    for cand in [cand1, cand2].iter().flatten() {
        if cand.exists() {
            let install_ctl = dirs::home_dir()
                .map(|h| h.join(".local/bin/rndctl"))
                .ok_or("Could not determine home directory")?;
            println!("Installing rndctl → {}", install_ctl.display());
            let bytes = fs::read(cand)?;
            let temp_ctl = install_ctl.with_extension("part");
            fs::write(&temp_ctl, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&temp_ctl)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&temp_ctl, perms)?;
            }
            fs::rename(&temp_ctl, &install_ctl)?;
            rndctl_installed = true;
            break;
        }
    }
    if !rndctl_installed {
        println!("rndctl not found nearby; skipping rndctl install.");
    }

    // Stop competing daemons
    for daemon in ["dunst", "mako"] {
        let active = Command::new("systemctl")
            .arg("--user")
            .arg("is-active")
            .arg("--quiet")
            .arg(daemon)
            .status()
            .ok()
            .map(|s| s.success())
            .unwrap_or(false);
        if active {
            println!("Stopping and masking {}...", daemon);
            let _ = Command::new("systemctl")
                .arg("--user")
                .arg("stop")
                .arg(daemon)
                .status();
            let _ = Command::new("systemctl")
                .arg("--user")
                .arg("mask")
                .arg(daemon)
                .status();
        }
    }

    if start {
        let _ = Command::new("systemctl")
            .arg("--user")
            .arg("enable")
            .arg("rnd.service")
            .status();
        let status = Command::new("systemctl")
            .arg("--user")
            .arg("restart")
            .arg("rnd.service")
            .status();
        if let Ok(s) = status {
            if s.success() {
                println!("rnd service started.");
            } else {
                println!("rnd service restart failed; check journalctl --user -u rnd -e");
            }
        }
    } else {
        println!("Install complete; skipped service start (--no-start).");
    }

    Ok(())
}

fn do_install_rndctl(path: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let cand_paths: Vec<PathBuf> = if let Some(p) = path {
        vec![p]
    } else {
        let exe = std::env::current_exe()?;
        let mut v = Vec::new();
        if let Some(parent) = exe.parent() {
            v.push(parent.join("rndctl"));
        }
        if let Ok(cwd) = std::env::current_dir() {
            v.push(cwd.join("target/release/rndctl"));
        }
        v
    };

    for cand in cand_paths.iter() {
        if cand.exists() {
            let install_ctl = dirs::home_dir()
                .map(|h| h.join(".local/bin/rndctl"))
                .ok_or("Could not determine home directory")?;
            println!("Installing rndctl → {}", install_ctl.display());
            let bytes = fs::read(cand)?;
            let temp_ctl = install_ctl.with_extension("part");
            fs::write(&temp_ctl, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&temp_ctl)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&temp_ctl, perms)?;
            }
            fs::rename(&temp_ctl, &install_ctl)?;
            return Ok(());
        }
    }

    Err("No rndctl binary found to install".into())
}

fn do_uninstall() -> Result<(), Box<dyn std::error::Error>> {
    // Disable and stop systemd service if present
    let _ = Command::new("systemctl")
        .arg("--user")
        .arg("disable")
        .arg("--now")
        .arg("rnd.service")
        .status();

    // Remove installed files
    if let Some(home) = dirs::home_dir() {
        let paths = [
            home.join(".local/bin/rnd"),
            home.join(".local/bin/rndctl"),
            home.join(".local/share/dbus-1/services/org.freedesktop.Notifications.service"),
            home.join(".config/systemd/user/rnd.service"),
        ];
        for p in paths.iter() {
            if p.exists() {
                let _ = fs::remove_file(p);
                println!("Removed {}", p.display());
            }
        }
    }

    // Reload systemd user units
    let _ = Command::new("systemctl")
        .arg("--user")
        .arg("daemon-reload")
        .status();

    Ok(())
}

fn activate(app: &Application, config: Arc<config::Config>) {
    load_css();

    // async_channel works with glib's MainContext executor on the GTK thread.
    // The D-Bus thread sends events; spawn_local polls the receiver on the
    // GTK main loop without any extra locking or bridging thread.
    let (event_tx, event_rx) = async_channel::bounded::<dbus::DaemonEvent>(64);

    let (signal_tx, signal_rx) = tokio::sync::mpsc::unbounded_channel::<dbus::DaemonSignal>();
    let (startup_tx, startup_rx) = sync_channel::<Result<(), String>>(1);

    let history = Arc::new(Mutex::new(history::History::new(
        config.general.history_size,
        config.general.persist_history,
    )));

    // ── D-Bus thread ─────────────────────────────────────────────────────────
    let history_for_dbus = Arc::clone(&history);
    std::thread::Builder::new()
        .name("dbus".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            runtime.block_on(async move {
                if let Err(e) = dbus::run(event_tx, signal_rx, startup_tx, history_for_dbus).await {
                    tracing::error!("D-Bus server error: {}", e);
                }
            });
        })
        .expect("Failed to spawn D-Bus thread");

    match startup_rx.recv() {
        Ok(Ok(())) => (),
        Ok(Err(err)) => {
            tracing::error!("Failed to start D-Bus server: {}", err);
            app.quit();
            return;
        }
        Err(e) => {
            tracing::error!("Failed to receive D-Bus startup status: {}", e);
            app.quit();
            return;
        }
    }

    // ── GTK side ─────────────────────────────────────────────────────────────
    let manager = app::NotificationManager::new(app, config, signal_tx, Arc::clone(&history));

    // spawn_local runs on the GTK main thread's glib executor, so Rc<RefCell<>>
    // is safe here — no Send requirement.
    glib::MainContext::default().spawn_local(async move {
        while let Ok(event) = event_rx.recv().await {
            app::NotificationManager::handle_event(&manager, event);
        }
        tracing::warn!("Event channel closed — D-Bus thread exited.");
    });

    tracing::info!("rnd active.");
}

// ── CSS ───────────────────────────────────────────────────────────────────────

fn load_css() {
    let display = gtk4::gdk::Display::default().expect("No GDK display");

    let default_provider = gtk4::CssProvider::new();
    default_provider.load_from_data(DEFAULT_CSS);
    gtk4::style_context_add_provider_for_display(
        &display,
        &default_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let user_css = config::Config::css_path();
    if user_css.exists() {
        let user_provider = gtk4::CssProvider::new();
        user_provider.load_from_path(&user_css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &user_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
        tracing::info!("Loaded user CSS from {:?}", user_css);
    }
}
