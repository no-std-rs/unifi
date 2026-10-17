//! Portable, no_std dashboard logic. The firmware supplies async HTTP and SPI.
#![no_std]

extern crate alloc;

pub mod api;
pub mod config;
pub mod navigation;
pub mod panel;
pub mod render;
pub mod stats;
pub mod tls;
