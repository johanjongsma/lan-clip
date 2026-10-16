//! `lan-clip install`: start the daemon at login with the platform's own
//! service manager.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn exe() -> Result<String> {
    Ok(std::env::current_exe()?.canonicalize()?.to_string_lossy().into_owned())
}

fn sh(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd).args(args).status().with_context(|| format!("running {cmd}"))?;
    anyhow::ensure!(status.success(), "{cmd} {} failed", args.join(" "));
    Ok(())
}

#[cfg(target_os = "linux")]
fn unit_path() -> PathBuf {
    crate::lanmouse::home().join(".config/systemd/user/lan-clip.service")
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<()> {
    let unit = format!(
        "[Unit]
Description=lan-clip: clipboard sync between lan-mouse machines
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart={} daemon
Restart=on-failure
RestartSec=3

[Install]
WantedBy=graphical-session.target
",
        exe()?
    );
    let path = unit_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, unit)?;
    sh("systemctl", &["--user", "daemon-reload"])?;
    sh("systemctl", &["--user", "enable", "--now", "lan-clip.service"])?;
    println!("installed {}", path.display());
    println!("allow the port from your LAN if you run a firewall, e.g.: sudo ufw allow 4243/tcp");
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> Result<()> {
    let _ = sh("systemctl", &["--user", "disable", "--now", "lan-clip.service"]);
    let _ = std::fs::remove_file(unit_path());
    sh("systemctl", &["--user", "daemon-reload"])?;
    println!("removed");
    Ok(())
}

#[cfg(target_os = "macos")]
const LABEL: &str = "lan-clip";

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    crate::lanmouse::home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn domain() -> Result<String> {
    let uid = String::from_utf8(Command::new("id").arg("-u").output()?.stdout)?;
    Ok(format!("gui/{}", uid.trim()))
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<()> {
    let log = crate::lanmouse::home().join("Library/Logs/lan-clip.log");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>daemon</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        exe()?,
        log.display()
    );
    let path = plist_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let _ = sh("launchctl", &["bootout", &format!("{}/{LABEL}", domain()?)]);
    std::fs::write(&path, plist)?;
    sh("launchctl", &["bootstrap", &domain()?, &path.to_string_lossy()])?;
    println!("installed {} (log: {})", path.display(), log.display());
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<()> {
    let _ = sh("launchctl", &["bootout", &format!("{}/{LABEL}", domain()?)]);
    let _ = std::fs::remove_file(plist_path());
    println!("removed");
    Ok(())
}
