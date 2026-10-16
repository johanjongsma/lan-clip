//! Wayland clipboard through wlr-data-control (Hyprland, Sway, river, ...).
//! Event driven: the compositor tells us about every new selection, no polling.

use super::{Backend, Change, Publish};
use crate::clip::{self, Clip};
use anyhow::{Context, Result};
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::{AsFd, OwnedFd};
use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

/// Set by KeePassXC, KDE and others on secrets.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";
const TEXT_ALIASES: [&str; 5] = [clip::TEXT, "UTF8_STRING", "text/plain", "STRING", "TEXT"];

struct State {
    conn: Connection,
    seat: WlSeat,
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    source: Option<ZwlrDataControlSourceV1>,
    tx: UnboundedSender<Change>,
    max: usize,
    /// The compositor announces the current selection on startup; that is
    /// not a new copy, so it is not sent.
    started: bool,
}

struct Handle {
    tx: mpsc::Sender<Clip>,
    wake: OwnedFd,
}

impl Publish for Handle {
    fn publish(&self, clip: Clip) {
        if self.tx.send(clip).is_ok() {
            let _ = rustix::io::write(&self.wake, &[1]);
        }
    }
}

pub fn spawn(max: usize) -> Result<Backend> {
    let conn = Connection::connect_to_env().context("connecting to Wayland (is WAYLAND_DISPLAY set?)")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=8, ())?;
    let manager: ZwlrDataControlManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .context("compositor lacks wlr-data-control (needs Hyprland, Sway, river, ...)")?;
    let device = manager.get_data_device(&seat, &qh, ());

    let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
    let (set_tx, set_rx) = mpsc::channel::<Clip>();
    let (wake_r, wake_w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK)?;

    let mut state = State { conn: conn.clone(), seat, manager, device, source: None, tx: change_tx, max, started: false };

    std::thread::Builder::new().name("wayland".into()).spawn(move || {
        let mut run = || -> Result<()> {
            loop {
                queue.dispatch_pending(&mut state)?;
                queue.flush()?;
                let Some(guard) = queue.prepare_read() else { continue };
                let (wayland_ready, wake_ready) = {
                    let conn_fd = guard.connection_fd();
                    let mut fds = [
                        PollFd::new(&conn_fd, PollFlags::IN),
                        PollFd::new(&wake_r, PollFlags::IN),
                    ];
                    poll(&mut fds, -1)?;
                    (!fds[0].revents().is_empty(), !fds[1].revents().is_empty())
                };
                if wayland_ready {
                    guard.read()?;
                } else {
                    drop(guard);
                }
                if wake_ready {
                    let mut buf = [0u8; 64];
                    while matches!(rustix::io::read(&wake_r, &mut buf), Ok(n) if n > 0) {}
                    while let Ok(clip) = set_rx.try_recv() {
                        state.publish(clip, &qh);
                    }
                }
            }
        };
        if let Err(e) = run() {
            eprintln!("lan-clip: wayland clipboard stopped: {e:#}");
            std::process::exit(1);
        }
    })?;

    Ok((change_rx, Box::new(Handle { tx: set_tx, wake: wake_w })))
}

impl State {
    fn publish(&mut self, clip: Clip, qh: &QueueHandle<Self>) {
        let source = self.manager.create_data_source(qh, Arc::new(clip.clone()));
        for (mime, _) in &clip.items {
            if mime == clip::TEXT {
                for alias in TEXT_ALIASES {
                    source.offer(alias.into());
                }
            } else {
                source.offer(mime.clone());
            }
        }
        self.device.set_selection(Some(&source));
        if let Some(old) = self.source.replace(source) {
            old.destroy();
        }
    }

    fn selection(&mut self, offer: ZwlrDataControlOfferV1) {
        if !std::mem::replace(&mut self.started, true) {
            offer.destroy();
            return;
        }
        let mimes = offer.data::<Mutex<Vec<String>>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
        if mimes.iter().any(|m| m == PASSWORD_HINT) {
            let _ = self.tx.send(Change::Concealed);
            offer.destroy();
            return;
        }
        let text = TEXT_ALIASES.iter().find(|a| mimes.iter().any(|m| m == *a));
        let mut wanted: Vec<(&str, String)> = Vec::new();
        if let Some(t) = text {
            wanted.push((clip::TEXT, t.to_string()));
        }
        for m in [clip::HTML, clip::PNG] {
            if mimes.iter().any(|x| x == m) {
                wanted.push((m, m.to_string()));
            }
        }
        if wanted.is_empty() {
            offer.destroy();
            return;
        }
        let mut pipes = Vec::new();
        for (ours, theirs) in wanted {
            let Ok((r, w)) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC) else { continue };
            offer.receive(theirs, w.as_fd());
            pipes.push((ours, r));
            // `w` drops here; the compositor has its own copy after the flush.
            let _ = self.conn.flush();
        }
        offer.destroy();

        // Read off the event thread: if the selection is our own source, the
        // data is served by this thread and reading here would deadlock.
        let tx = self.tx.clone();
        let max = self.max;
        std::thread::spawn(move || {
            let mut clip = Clip::default();
            for (mime, fd) in pipes {
                let mut data = Vec::new();
                let mut f = std::fs::File::from(fd).take(max as u64 + 1);
                if f.read_to_end(&mut data).is_ok() && !data.is_empty() && data.len() <= max {
                    clip.items.push((mime.to_string(), data));
                }
            }
            if !clip.is_empty() {
                let _ = tx.send(Change::Copied(clip));
            }
        });
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrDataControlManagerV1, _: <ZwlrDataControlManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(state: &mut Self, device: &ZwlrDataControlDeviceV1, event: zwlr_data_control_device_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        use zwlr_data_control_device_v1::Event;
        match event {
            Event::Selection { id: Some(offer) } => state.selection(offer),
            Event::Selection { id: None } => state.started = true,
            Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
            Event::Finished => {
                device.destroy();
                state.device = state.manager.get_data_device(&state.seat, qh, ());
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, Mutex::new(Vec::new())),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, Mutex<Vec<String>>> for State {
    fn event(_: &mut Self, _: &ZwlrDataControlOfferV1, event: zwlr_data_control_offer_v1::Event, mimes: &Mutex<Vec<String>>, _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            mimes.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, Arc<Clip>> for State {
    fn event(state: &mut Self, source: &ZwlrDataControlSourceV1, event: zwlr_data_control_source_v1::Event, clip: &Arc<Clip>, _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_data_control_source_v1::Event;
        match event {
            Event::Send { mime_type, fd } => {
                let key = if TEXT_ALIASES.contains(&mime_type.as_str()) { clip::TEXT } else { mime_type.as_str() };
                let data = clip.get(key).map(<[u8]>::to_vec).unwrap_or_default();
                std::thread::spawn(move || {
                    let _ = std::fs::File::from(fd).write_all(&data);
                });
            }
            Event::Cancelled => {
                if state.source.as_ref() == Some(source) {
                    state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}
