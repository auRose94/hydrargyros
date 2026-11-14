// Bakes the manifest's identity at build time: tests rebuild this plugin
// with different env values to make ONE source produce DIFFERENT
// versions — the hot-swap story needs v1 and v2 of the same plugin.
fn main() {
    println!("cargo:rerun-if-env-changed=HYG_PLUGIN_VERSION");
    println!("cargo:rerun-if-env-changed=HYG_PLUGIN_ID");
    println!("cargo:rerun-if-env-changed=HYG_PLUGIN_ABI");

    let version = std::env::var("HYG_PLUGIN_VERSION").unwrap_or_else(|_| "0.1.0".into());
    println!("cargo:rustc-env=HYG_PLUGIN_VERSION={version}");

    let id = std::env::var("HYG_PLUGIN_ID").unwrap_or_else(|_| "greeter".into());
    println!("cargo:rustc-env=HYG_PLUGIN_ID={id}");

    // the ABI lie rides through only when set — an honest build reports
    // the contract constant and never sets this env
    if let Ok(abi) = std::env::var("HYG_PLUGIN_ABI") {
        println!("cargo:rustc-env=HYG_PLUGIN_ABI={abi}");
    }
}
