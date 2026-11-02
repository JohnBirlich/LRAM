//! Roblox clients LRAM started: the multi-instance lock, process tracking, log watching
//! and closing. Nothing here reads or writes another process's memory or handles; anti-AFK
//! (`afk`) presses a key the way a keyboard does.

pub mod afk;
pub mod audio;
mod autopsy;
mod capture;
mod gpu;
pub mod limits;
pub mod logwatch;
pub mod metrics;
pub mod multi;
pub mod process;
pub mod qos;
pub mod resize;
pub mod window;
mod tracker;

pub mod rejoin;
pub use tracker::{Tracker, Transition, auth_failed, challenged, closed, hung, unexpected, wrong_account};
