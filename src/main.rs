mod clip;
mod clipboard;
mod config;
mod daemon;
mod lanmouse;
mod service;
mod tls;

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};

const USAGE: &str = "\
lan-clip — the clipboard lan-mouse is missing

usage:
  lan-clip daemon            run the sync daemon (normally via install)
  lan-clip status [--json]   show peers and the last synced clip
  lan-clip pause | resume | toggle
  lan-clip install           run at login (systemd user unit / launchd agent)
  lan-clip uninstall         remove that again
";

fn main() {
    if let Err(e) = run() {
        eprintln!("lan-clip: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("daemon") => {
            let cfg = config::Config::load()?;
            tokio::runtime::Runtime::new()?.block_on(daemon::run(cfg))
        }
        Some("status") => {
            let reply = control("status")?;
            if args.iter().any(|a| a == "--json") {
                println!("{reply}");
            } else {
                print_status(&reply)?;
            }
            Ok(())
        }
        Some(cmd @ ("pause" | "resume" | "toggle")) => {
            let v: serde_json::Value = serde_json::from_str(&control(cmd)?)?;
            println!("{}", if v["paused"].as_bool() == Some(true) { "paused" } else { "syncing" });
            Ok(())
        }
        Some("install") => service::install(),
        Some("uninstall") => service::uninstall(),
        Some("-V" | "--version" | "version") => {
            println!("lan-clip {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn control(cmd: &str) -> Result<String> {
    let path = daemon::control_path();
    let mut s = std::os::unix::net::UnixStream::connect(&path)
        .with_context(|| format!("lan-clip daemon is not running ({})", path.display()))?;
    writeln!(s, "{cmd}")?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn print_status(json: &str) -> Result<()> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let ago = |t: &serde_json::Value| t.as_u64().map(|t| format!("{}s ago", now.saturating_sub(t))).unwrap_or("never".into());
    println!("lan-clip {}  {}  port {}", v["version"].as_str().unwrap_or("?"),
        if v["paused"].as_bool() == Some(true) { "PAUSED" } else { "syncing" }, v["port"]);
    for p in v["peers"].as_array().into_iter().flatten() {
        let state = if p["reachable"].as_bool() == Some(true) { "up" } else { "down" };
        print!("  {:<24} {:<5} {}", p["name"].as_str().unwrap_or("?"), state, p["address"].as_str().unwrap_or("-"));
        if let Some(e) = p["error"].as_str() {
            print!("  ({e})");
        }
        println!();
    }
    if let Some(l) = v["last"].as_object() {
        println!("  last: {} {} {} {} — {}", l["direction"].as_str().unwrap_or(""), l["kind"].as_str().unwrap_or(""),
            if l["direction"] == "sent" { "to" } else { "from" }, l["peer"].as_str().unwrap_or(""), ago(&l["at"]));
        println!("        {}", l["summary"].as_str().unwrap_or(""));
    }
    if let Some(n) = v["concealed_skipped"].as_u64().filter(|n| *n > 0) {
        println!("  {n} password-manager copies kept local");
    }
    Ok(())
}
