//! macOS pasteboard. AppKit has no change notification, so the change counter
//! is checked four times a second, which is what every Mac clipboard tool does.
//! Copies arriving from an iPhone through Universal Clipboard show up here too.

use super::{Backend, Change, Publish};
use crate::clip::{self, Clip};
use anyhow::Result;
use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use std::sync::mpsc;
use std::time::Duration;

/// nspasteboard.org convention used by 1Password, Bitwarden, KeePassXC, ...
const CONCEALED: &str = "org.nspasteboard.ConcealedType";
const JPEG: &str = "public.jpeg";

struct Handle(mpsc::Sender<Clip>);

impl Publish for Handle {
    fn publish(&self, clip: Clip) {
        let _ = self.0.send(clip);
    }
}

pub fn spawn(max: usize) -> Result<Backend> {
    let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
    let (set_tx, set_rx) = mpsc::channel::<Clip>();

    std::thread::Builder::new().name("pasteboard".into()).spawn(move || {
        let pb = NSPasteboard::generalPasteboard();
        let mut last = pb.changeCount();
        loop {
            autoreleasepool(|_| {
                while let Ok(clip) = set_rx.try_recv() {
                    write(&pb, &clip);
                    last = pb.changeCount();
                }
                let now = pb.changeCount();
                if now != last {
                    last = now;
                    if let Some(change) = read(&pb, max) {
                        let _ = change_tx.send(change);
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(250));
        }
    })?;

    Ok((change_rx, Box::new(Handle(set_tx))))
}

fn read(pb: &NSPasteboard, max: usize) -> Option<Change> {
    let types: Vec<String> = pb.types().map(|t| t.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    if types.iter().any(|t| t == CONCEALED) {
        return Some(Change::Concealed);
    }
    let mut clip = Clip::default();
    unsafe {
        if let Some(s) = pb.stringForType(NSPasteboardTypeString) {
            clip.items.push((clip::TEXT.into(), s.to_string().into_bytes()));
        }
        if let Some(d) = pb.dataForType(NSPasteboardTypeHTML) {
            clip.items.push((clip::HTML.into(), d.to_vec()));
        }
        let png = pb.dataForType(NSPasteboardTypePNG).map(|d| d.to_vec()).or_else(|| {
            // Screenshots and iPhone photos often come as TIFF or JPEG only;
            // Linux apps expect PNG.
            pb.dataForType(NSPasteboardTypeTIFF)
                .or_else(|| pb.dataForType(&NSString::from_str(JPEG)))
                .and_then(|d| to_png(&d))
        });
        if let Some(png) = png {
            clip.items.push((clip::PNG.into(), png));
        }
    }
    clip.items.retain(|(_, d)| !d.is_empty());
    if clip.is_empty() || clip.size() > max {
        return None;
    }
    Some(Change::Copied(clip))
}

fn to_png(data: &NSData) -> Option<Vec<u8>> {
    let rep: Retained<NSBitmapImageRep> = NSBitmapImageRep::imageRepWithData(data)?;
    let props = NSDictionary::new();
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(png.to_vec())
}

fn write(pb: &NSPasteboard, clip: &Clip) {
    pb.clearContents();
    let mut types = Vec::new();
    unsafe {
        for (mime, _) in &clip.items {
            match mime.as_str() {
                clip::TEXT => types.push(NSPasteboardTypeString),
                clip::HTML => types.push(NSPasteboardTypeHTML),
                clip::PNG => types.push(NSPasteboardTypePNG),
                _ => {}
            }
        }
        pb.declareTypes_owner(&NSArray::from_slice(&types), None);
        for (mime, data) in &clip.items {
            match mime.as_str() {
                clip::TEXT => {
                    let s = NSString::from_str(&String::from_utf8_lossy(data));
                    pb.setString_forType(&s, NSPasteboardTypeString);
                }
                clip::HTML => {
                    pb.setData_forType(Some(&NSData::with_bytes(data)), NSPasteboardTypeHTML);
                }
                clip::PNG => {
                    pb.setData_forType(Some(&NSData::with_bytes(data)), NSPasteboardTypePNG);
                }
                _ => {}
            }
        }
    }
}
