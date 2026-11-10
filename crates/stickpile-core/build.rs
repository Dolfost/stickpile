//! Map the `wayland` / `x11` clipboard features to cfgs that are only set on
//! Linux, where their (target-specific) dependencies exist.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(wl_backend)");
    println!("cargo::rustc-check-cfg=cfg(x11_backend)");
    let linux = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux");
    if linux && std::env::var_os("CARGO_FEATURE_WAYLAND").is_some() {
        println!("cargo::rustc-cfg=wl_backend");
    }
    if linux && std::env::var_os("CARGO_FEATURE_X11").is_some() {
        println!("cargo::rustc-cfg=x11_backend");
    }
}
