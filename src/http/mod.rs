//! 统一 HTTP 客户端适配器。

pub mod client;
pub mod error;
pub mod policy;

pub(crate) mod middleware;

#[cfg(test)]
pub mod mock;

pub use client::{HttpClient, HttpClientBuilder, HttpClientError};
pub use error::HttpError;
pub use policy::RetryPolicy;
