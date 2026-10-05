//! Munchiko Calendar — the planning core for Mochi & Momo's account.
//!
//! The crate is split into layers:
//! - [`models`]: the things the app knows about (fabric, accessories, events, post plans).
//! - [`planner`]: the scheduling engine that works backward from a post date.
//! - [`db`]: SQLite storage that loads and saves the model types.
//!
//! `models` and `planner` know nothing about databases or web servers; the
//! outer layers only *use* their types. That keeps the core logic easy to test.

pub mod db;
pub mod models;
pub mod planner;
