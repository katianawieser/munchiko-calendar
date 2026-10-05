//! Munchiko Calendar — the planning core for Mochi & Momo's account.
//!
//! The crate is split into layers:
//! - [`models`]: the things the app knows about (fabric, accessories, events, post plans).
//! - [`planner`]: the scheduling engine that works backward from a post date.
//! - [`holidays`]: reads the holidays file and works out each holiday's dates.
//! - [`db`]: SQLite storage that loads and saves the model types.
//! - [`web`]: the Axum web app that shows it all in the browser.
//! - [`demo`]: the example Halloween post.
//!
//! `models`, `planner` and `holidays` know nothing about databases or web servers; the
//! outer layers only *use* their types. That keeps the core logic easy to test.

pub mod db;
pub mod demo;
pub mod holidays;
pub mod models;
pub mod planner;
pub mod web;
