#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

#[cfg(any(feature = "std", feature = "std_async", feature = "async_std"))]
extern crate std;

#[cfg(feature = "async_std")]
pub mod async_std;
#[cfg(feature = "std")]
pub mod blocking;
#[cfg(feature = "embassy")]
pub mod embassy;
#[cfg(any(feature = "std", feature = "std_async", feature = "async_std"))]
pub mod host;
#[cfg(feature = "std_async")]
pub mod std_async;

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
