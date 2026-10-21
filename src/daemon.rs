//! The daemon: watches the local clipboard, pushes new clips to every
//! lan-mouse peer, accepts clips from them, and answers the control socket.

use crate::clip::Clip;
use crate::clipboard::{self, Change, Publish};
use crate::{config::Config, lanmouse, tls};
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use tokio::sync::Semaphore;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_EVERY: Duration = Duration::from_secs(30);
/// Incoming transfers handled at once; further connections are dropped.
const MAX_INCOMING: usize = 4;

#[derive(Serialize, Clone, Default)]
pub struct PeerStatus {
    pub name: String,
    pub address: Option<String>,
    pub reachable: bool,
    pub last_seen: Option<u64>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct LastSync {
    pub direction: &'static str,
    pub peer: String,
    pub kind: &'static str,
    pub summary: String,
    pub at: u64,
}

#[derive(Serialize, Default)]
pub struct Status {
    pub version: &'static str,
    pub paused: bool,
    pub port: u16,
    pub fingerprint: String,
    pub peers: Vec<PeerStatus>,
    pub last: Option<LastSync>,
    pub concealed_skipped: u64,
    #[serde(skip)]
    last_hash: Option<[u8; 32]>,
}

type Shared = Arc<Mutex<Status>>;

struct Ctx {
    cfg: Config,
    status: Shared,
    peers: Vec<lanmouse::Peer>,
    connector: TlsConnector,
    clipboard: Box<dyn Publish>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub async fn run(cfg: Config) -> Result<()> {
    let (client_tls, server_tls) = tls::configs()?;
    let peers = lanmouse::peers()?;
    if peers.is_empty() {
        eprintln!("lan-clip: lan-mouse has no clients configured; only accepting incoming clips");
    }
    let (mut changes, clipboard) = clipboard::spawn(cfg.max_bytes)?;

    let status = Arc::new(Mutex::new(Status {
        version: env!("CARGO_PKG_VERSION"),
        port: cfg.port,
        fingerprint: lanmouse::fingerprint(&lanmouse::identity()?.cert),
        peers: peers
            .iter()
            .map(|p| PeerStatus { name: p.hostname.clone(), ..Default::default() })
            .collect(),
        ..Default::default()
    }));

    let listener = TcpListener::bind(("::", cfg.port))
        .await
        .or(TcpListener::bind(("0.0.0.0", cfg.port)).await)
        .with_context(|| format!("listening on port {}", cfg.port))?;
    let control = control_listener()?;

    let ctx = Arc::new(Ctx { cfg, status, peers, connector: TlsConnector::from(client_tls), clipboard });
    eprintln!("lan-clip {}: listening on {}, {} peer(s)", env!("CARGO_PKG_VERSION"), ctx.cfg.port, ctx.peers.len());

    let acceptor = TlsAcceptor::from(server_tls);
    let slots = Arc::new(Semaphore::new(MAX_INCOMING));
    let c = ctx.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, from)) = listener.accept().await else { continue };
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                eprintln!("lan-clip: incoming from {from}: too many transfers in progress, dropped");
                continue;
            };
            let (c, acceptor) = (c.clone(), acceptor.clone());
            tokio::spawn(async move {
                // The whole transfer gets the time a sender of a max_bytes clip is given.
                let deadline = CONNECT_TIMEOUT + Duration::from_secs((c.cfg.max_bytes / 2_000_000) as u64);
                match tokio::time::timeout(deadline, receive(&c, &acceptor, tcp)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("lan-clip: incoming from {from}: {e:#}"),
                    Err(_) => eprintln!("lan-clip: incoming from {from}: timed out"),
                }
                drop(slot);
            });
        }
    });

    let c = ctx.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = control.accept().await else { continue };
            let c = c.clone();
            tokio::spawn(async move {
                let _ = answer(&c, stream).await;
            });
        }
    });

    let c = ctx.clone();
    tokio::spawn(async move {
        loop {
            send_all(&c, &Clip::default()).await;
            tokio::time::sleep(PROBE_EVERY).await;
        }
    });

    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Some(Change::Copied(clip)) => local_copy(&ctx, clip).await,
                Some(Change::Concealed) => ctx.status.lock().unwrap().concealed_skipped += 1,
                None => anyhow::bail!("clipboard backend stopped"),
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let _ = std::fs::remove_file(control_path());
    Ok(())
}

async fn local_copy(ctx: &Arc<Ctx>, clip: Clip) {
    let hash = clip.hash();
    {
        let mut s = ctx.status.lock().unwrap();
        // Same content we just received from a peer (or already sent): no echo.
        if s.last_hash == Some(hash) || s.paused {
            s.last_hash = Some(hash);
            return;
        }
        s.last_hash = Some(hash);
    }
    let sent_to = send_all(ctx, &clip).await;
    if !sent_to.is_empty() {
        ctx.status.lock().unwrap().last = Some(LastSync {
            direction: "sent",
            peer: sent_to.join(", "),
            kind: clip.kind(),
            summary: clip.summary(),
            at: now(),
        });
    }
}

/// Sends `clip` to every peer in parallel; an empty clip is a reachability probe.
async fn send_all(ctx: &Arc<Ctx>, clip: &Clip) -> Vec<String> {
    let clip = Arc::new(clip.clone());
    let tasks: Vec<_> = (0..ctx.peers.len())
        .map(|i| {
            let (ctx, clip) = (ctx.clone(), clip.clone());
            tokio::spawn(async move { (i, send_one(&ctx, i, &clip).await) })
        })
        .collect();
    let mut ok = Vec::new();
    for t in tasks {
        let Ok((i, result)) = t.await else { continue };
        let mut s = ctx.status.lock().unwrap();
        let p = &mut s.peers[i];
        match result {
            Ok(addr) => {
                p.reachable = true;
                p.address = Some(addr);
                p.last_seen = Some(now());
                p.error = None;
                ok.push(p.name.clone());
            }
            Err(e) => {
                p.reachable = false;
                p.error = Some(format!("{e:#}"));
            }
        }
    }
    ok
}

async fn send_one(ctx: &Ctx, i: usize, clip: &Clip) -> Result<String> {
    let peer = &ctx.peers[i];
    let known = ctx.status.lock().unwrap().peers[i].address.clone();
    let mut candidates: Vec<String> = known.into_iter().collect();
    candidates.extend(peer.ips.iter().cloned());
    if !peer.hostname.is_empty() {
        candidates.push(peer.hostname.clone());
    }
    candidates.dedup();
    let mut errors = Vec::new();
    for addr in candidates {
        let attempt = async {
            let tcp = TcpStream::connect((addr.as_str(), ctx.cfg.port)).await?;
            let name = rustls::pki_types::ServerName::try_from("lan-clip")?;
            let mut tls = ctx.connector.connect(name, tcp).await?;
            clip.write(&mut tls).await?;
            let ack = tls.read_u8().await?;
            anyhow::ensure!(ack == 1, "peer refused the clip");
            let _ = tls.shutdown().await;
            Ok::<_, anyhow::Error>(())
        };
        match tokio::time::timeout(CONNECT_TIMEOUT + Duration::from_secs((clip.size() / 2_000_000) as u64), attempt).await {
            Ok(Ok(())) => return Ok(addr),
            Ok(Err(e)) => errors.push(format!("{addr}: {e:#}")),
            Err(_) => errors.push(format!("{addr}: timed out")),
        }
    }
    if errors.is_empty() {
        anyhow::bail!("no address for {}", peer.hostname);
    }
    anyhow::bail!(errors.join("; "))
}

async fn receive(ctx: &Ctx, acceptor: &TlsAcceptor, tcp: TcpStream) -> Result<()> {
    let mut tls = tokio::time::timeout(CONNECT_TIMEOUT, acceptor.accept(tcp)).await??;
    let fp = tls::peer_fingerprint(tls.get_ref().1.peer_certificates()).unwrap_or_default();
    let name = lanmouse::authorized().get(&fp).cloned().unwrap_or(fp);
    let clip = Clip::read(&mut tls, ctx.cfg.max_bytes).await?;
    tls.write_u8(1).await?;
    let _ = tls.shutdown().await;

    let mut s = ctx.status.lock().unwrap();
    if let Some(p) = s.peers.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&name)) {
        p.reachable = true;
        p.last_seen = Some(now());
        p.error = None;
    }
    if clip.is_empty() || s.paused {
        return Ok(());
    }
    let hash = clip.hash();
    if s.last_hash == Some(hash) {
        return Ok(());
    }
    s.last_hash = Some(hash);
    s.last = Some(LastSync { direction: "received", peer: name, kind: clip.kind(), summary: clip.summary(), at: now() });
    drop(s);
    ctx.clipboard.publish(clip);
    Ok(())
}

pub fn control_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("lan-clip.sock")
}

fn control_listener() -> Result<UnixListener> {
    let path = control_path();
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        anyhow::bail!("lan-clip is already running ({})", path.display());
    }
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))
}

async fn answer(ctx: &Ctx, stream: tokio::net::UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await?;
    let reply = {
        let mut s = ctx.status.lock().unwrap();
        match line.trim() {
            "pause" => s.paused = true,
            "resume" => s.paused = false,
            "toggle" => s.paused = !s.paused,
            _ => {}
        }
        serde_json::to_string(&*s)?
    };
    w.write_all(reply.as_bytes()).await?;
    w.write_all(b"\n").await?;
    Ok(())
}
