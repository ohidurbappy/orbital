//! `Cargo.toml` is the source of truth for the version — Cargo bakes it into
//! the binary, so `VERSION` is fixed at build time with no codegen step.
//!
//! CI overrides it with `ORBITAL_VERSION` (`MAJOR.MINOR` from Cargo.toml plus
//! the workflow run number as the patch), so every push ships a
//! strictly-increasing version without anyone editing a file by hand.

pub const VERSION: &str = match option_env!("ORBITAL_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// `owner/repo` used for release checks and downloads.
pub const REPO: &str = "ohidurbappy/orbital";

/// The name the binary is installed as, used in help text and asset names.
pub const BIN: &str = "orbital";

/// `User-Agent` sent with every request we make to GitHub.
pub fn user_agent() -> String {
    format!("{BIN}/{VERSION}")
}
