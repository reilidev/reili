//! Work State persistence on GitHub Issues.

mod codec;
#[cfg(test)]
mod fake_github;
mod repository;
#[cfg(test)]
mod tests;

pub use repository::{GitHubWorkStateConfig, GitHubWorkStateRepository};
