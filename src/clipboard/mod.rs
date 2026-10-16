//! Platform clipboard access. Each backend runs on its own thread, reports
//! every new clipboard owner as a `Change`, and accepts clips to publish.

use crate::clip::Clip;
use tokio::sync::mpsc::UnboundedReceiver;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::spawn;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::spawn;

pub enum Change {
    Copied(Clip),
    /// A password manager marked the copy as secret; it is never sent.
    Concealed,
}

pub trait Publish: Send + Sync {
    fn publish(&self, clip: Clip);
}

pub type Backend = (UnboundedReceiver<Change>, Box<dyn Publish>);
