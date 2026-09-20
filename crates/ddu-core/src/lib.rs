//! ddu's domain layer: the project/session model ([`session`]) and the
//! two files the app persists ([`config`]: `settings.json` +
//! `state.json`).
//!
//! Nothing here draws and nothing here spawns: the model names the
//! commands a session runs (`AgentCmd::spec`), [`config`] reads and
//! writes them, and the shell decides when.

pub mod config;
pub mod session;
