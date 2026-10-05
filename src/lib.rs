//! Munchiko Calendar — the planning core for Mochi & Momo's account.
//!
//! This crate is split into two layers:
//! - [`models`]: the things the app knows about (fabric, accessories, events, post plans).
//! - [`planner`]: the scheduling engine that works backward from a post date.
//!
//! Neither layer knows about databases or web servers; those come in later
//! milestones and only *use* these types. That keeps the core logic easy to test.

pub mod models;
pub mod planner;
