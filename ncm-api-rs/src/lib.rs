//! Netease Cloud Music API - Rust SDK
//!
//! 原项目: <https://github.com/NeteaseCloudMusicApiEnhanced/api-enhanced>
//! 从 Node.js 版本移植的 Rust 原生实现
//! 支持 weapi / eapi / linuxapi 三种加密方式

#![deny(unsafe_code)]

pub mod api;
pub mod crypto;
pub mod error;
pub mod request;
pub mod util;

pub use api::Query;
pub use error::NcmError;
pub use request::{ApiClient, ApiResponse, CryptoType, RequestOption};
