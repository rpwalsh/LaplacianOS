#![no_std]
#![warn(missing_docs)]

//! LaplacianOS UI SDK - minimal substrate + toolkit for LaplacianOS-native UI.
//!
//! Built on top of `laplacianos-app-sdk`. All types are `no_std`, heap-free where
//! possible, and renderable directly onto a `laplacianos_app_sdk::canvas::Canvas`.

pub mod charts;
pub mod geom;
pub mod interactive;
pub mod native_views;
pub mod substrate;
pub mod tokens;
pub mod toolkit;
pub mod widgets;
