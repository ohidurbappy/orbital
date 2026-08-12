fn main() {
    // CI stamps the release version through the environment (see
    // src/core/version.rs), so a cached build must be redone when it changes.
    println!("cargo:rerun-if-env-changed=ORBITAL_VERSION");
}
