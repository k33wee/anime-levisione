//! Portable core of anime-levisione: streaming providers, the AniList
//! client, and the library/config store. The terminal UI lives in the
//! sibling `anime-levisione` crate; mobile apps consume this core through
//! the `uniffi` feature (see `api`).

pub mod anilist;
pub mod audio;
pub mod model;
pub mod provider;
pub mod store;

/// FFI surface for the Android app: `--features uniffi`.
#[cfg(feature = "uniffi")]
pub mod api;

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();
