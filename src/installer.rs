use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use crate::*;

pub fn do_install(start: bool) -> Result<(), Box<dyn std::error::Error>> {
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
    let mut perms = fs::metadata(&temp_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&temp_path, perms)?;
    fs::rename(&temp_path, &install_bin)?;

    // systemd unit
    let systemd_dir = dirs::home_dir()
        .map(|h| h.join(".config/systemd/user"))
        .ok_or("Could not determine home directory")?;
    fs::create_dir_all(&systemd_dir)?;
    let service_contents =
        RND_SERVICE_SRC.replace("%h/.cargo/bin/rnd", &install_bin.to_string_lossy());
    let service_path = systemd_dir.join("rnd.service");
    fs::write(&service_path, service_contents)?;

    // D-Bus activation file
    let dbus_dir = dirs::home_dir()
        .map(|h| h.join(".local/share/dbus-1/services"))
        .ok_or("Could not determine home directory")?;
    fs::create_dir_all(&dbus_dir)?;
    let dbus_contents = DBUS_SRC.replace("@@RND_BIN@@", &install_bin.to_string_lossy());
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
            let mut perms = fs::metadata(&temp_ctl)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&temp_ctl, perms)?;
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

pub fn do_install_rndctl(path: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
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
            let mut perms = fs::metadata(&temp_ctl)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&temp_ctl, perms)?;
            fs::rename(&temp_ctl, &install_ctl)?;
            return Ok(());
        }
    }

    Err("No rndctl binary found to install".into())
}

pub fn do_uninstall() -> Result<(), Box<dyn std::error::Error>> {
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
