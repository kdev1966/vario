fn main() {
    // Slint tracks the .slint files; the translations must be tracked too.
    println!("cargo:rerun-if-changed=lang");
    let config = slint_build::CompilerConfiguration::new().with_bundled_translations("lang");
    slint_build::compile_with_config("ui/app.slint", config).expect("the interface compiles");
}
