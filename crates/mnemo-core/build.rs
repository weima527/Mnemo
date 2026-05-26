//! Build script for `mnemo-core`.
//!
//! Workaround for a `libgit2-sys 0.17` linking gap on `x86_64-pc-windows-msvc`:
//! libgit2 references Win32 security / registry / CryptoAPI functions
//! (`OpenProcessToken`, `RegOpenKeyExW`, `CryptAcquireContextA`, …) that live in
//! `advapi32.lib`, but `libgit2-sys` does not emit a link directive for it, so
//! every downstream binary fails with `LNK2019` unresolved externals.
//!
//! `mnemo-core` owns the (default-on) `git2` dependency and sits below every
//! other crate, so emitting the link directive here resolves it for the whole
//! workspace. Native link directives are propagated to the final linked
//! artifact. Revisit / remove when `git2` is bumped past the fixed version.

fn main() {
    let is_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let is_msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if is_windows && is_msvc {
        println!("cargo:rustc-link-lib=dylib=advapi32");
    }
}
