//! Portable, no_std dashboard logic. The firmware supplies async HTTP and SPI.
#![no_std]

extern crate alloc;
pub mod certificate;

pub mod api;
pub mod app;
pub mod config;
pub mod console_http;
pub mod navigation;
pub mod panel;
pub mod provision;
pub mod render;
pub mod settings;
pub mod stats;
pub mod tls;
