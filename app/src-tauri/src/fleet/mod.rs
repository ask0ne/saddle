pub mod events;
pub mod registry;
pub mod store;
pub mod transcript;
pub mod watch;

use std::path::PathBuf;

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
