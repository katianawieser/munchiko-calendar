//! Munchiko Calendar — the planning core for Mochi & Momo's account.
//!
//! The crate is split into layers:
//! - [`models`]: the things the app knows about (fabric, accessories, events, post plans).
//! - [`planner`]: the scheduling engine that works backward from a post date.
//! - [`holidays`]: reads the holidays file and works out each holiday's dates.
//! - [`suggest`]: "for Earth Day, consider the leaf bow", with reasons.
//! - [`photos`]: recognises image files and keeps them in a folder on disk.
//! - [`db`]: SQLite storage that loads and saves the model types.
//! - [`web`]: the Axum web app that shows it all in the browser.
//! - [`demo`]: the example Halloween post.
//!
//! `models`, `planner`, `holidays` and `suggest` know nothing about databases or web servers; the
//! outer layers only *use* their types. That keeps the core logic easy to test.

pub mod db;
pub mod demo;
pub mod holidays;
pub mod models;
pub mod photos;
pub mod planner;
pub mod suggest;
pub mod web;
