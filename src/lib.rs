#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod client;
pub mod json;
pub mod multipart;
pub mod network;
pub mod protect;
pub mod request;
pub mod session;
pub mod session_api;
pub mod site_manager;
#[doc(hidden)]
pub mod validation;
pub mod websocket;

pub use client::{Authentication, Body, Client, Error, HttpError, Response};
pub use json::{Json, JsonError, JsonString};
pub use request::{BuildError, Call, Operation, QueryParameter};
pub use reqwless::request::Method;
