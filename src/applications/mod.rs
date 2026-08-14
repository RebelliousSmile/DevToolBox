//! Application recommendation support shared by the Python report and egui.

#![allow(dead_code)]

use std::io;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
pub mod usage;
#[cfg(windows)]
mod windows;

#[allow(unused_imports)]
pub use usage::{UsageService, UsageTarget};

pub trait ProcessProvider: Send + Sync + 'static {
    fn executable_paths(&self) -> io::Result<Vec<PathBuf>>;
}

pub struct SystemProcessProvider;

impl ProcessProvider for SystemProcessProvider {
    fn executable_paths(&self) -> io::Result<Vec<PathBuf>> {
        #[cfg(target_os = "linux")]
        {
            linux::executable_paths()
        }
        #[cfg(windows)]
        {
            windows::executable_paths()
        }
    }
}
