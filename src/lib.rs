//! Protocol, device access and formatting for the Ecowitt WH6006 weather
//! logger, shared by the `wh6006` CLI and the `wh6006-gui` desktop app.

pub mod device;
pub mod firmware;
pub mod form;
pub mod hid;
pub mod model;
pub mod protocol;
pub mod units;
pub mod view;
