use std::env;

fn main() {
    println!(
        "cargo:rustc-env=SENDIT_PROFILE={}",
        env::var("PROFILE").unwrap()
    );
    println!(
        "cargo:rustc-env=SENDIT_TARGET={}",
        env::var("TARGET").unwrap()
    );
}
