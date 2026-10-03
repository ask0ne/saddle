//! The only place `unsafe` or os-specific code may live (see
//! ARCHITECTURE.md, The FFI gap) — cfg-gated so a future non-macOS backend
//! has a seam to land in.

#[cfg(target_os = "macos")]
pub mod panel;
#[cfg(target_os = "macos")]
pub mod terminal;
