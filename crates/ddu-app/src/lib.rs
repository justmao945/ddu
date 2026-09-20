//! The shell: [`app::AppView`] — the workspace state, the frame and the
//! action handlers — plus the panels it draws ([`ui`]).
//!
//! The four surface regions are `impl AppView` blocks living in [`ui`],
//! which is why the two modules share one crate: a panel takes the view
//! itself, so the view cannot sit below the panels.

pub mod app;
pub mod ui;
