use rnd::*;

use std::env;
use std::path::PathBuf;
use std::sync::{mpsc::sync_channel, Arc, Mutex};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::Application;

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

        if let Err(e) = installer::do_uninstall() {
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
        if let Err(e) = installer::do_install_rndctl(path) {
            eprintln!("rndctl install failed: {}", e);
            std::process::exit(1);
        }
        println!("rndctl install completed.");
        return;
    }

    // Install the contrib files
    if args.iter().any(|a| a == "--install") {
        let no_start = args.iter().any(|a| a == "--no-start");
        if let Err(e) = installer::do_install(!no_start) {
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

    // D-Bus thread
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

    // GTK side
    let manager = app::NotificationManager::new(app, config, signal_tx, Arc::clone(&history));

    glib::MainContext::default().spawn_local(async move {
        while let Ok(event) = event_rx.recv().await {
            app::NotificationManager::handle_event(&manager, event);
        }
        tracing::warn!("Event channel closed — D-Bus thread exited.");
    });

    tracing::info!("rnd active.");
}

// CSS

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
