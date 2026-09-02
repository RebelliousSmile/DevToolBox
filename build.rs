use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=DEVTOOLBOX_UPDATE_PUBLIC_KEYS");
    println!("cargo:rerun-if-env-changed=DEVTOOLBOX_RELEASE_BUILD");
    let path = env::var_os("DEVTOOLBOX_UPDATE_PUBLIC_KEYS").map(PathBuf::from);
    if let Some(path) = &path {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let json = path
        .as_ref()
        .map(fs::read_to_string)
        .transpose()
        .unwrap_or_else(|error| panic!("cannot read updater public keyring: {error}"))
        .unwrap_or_else(|| "{}".to_string());
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("invalid updater public keyring JSON: {error}"));
    let configured = parsed.as_object().is_some_and(|keys| !keys.is_empty());
    if env::var("DEVTOOLBOX_RELEASE_BUILD").as_deref() == Ok("1") && !configured {
        panic!("stable release build requires DEVTOOLBOX_UPDATE_PUBLIC_KEYS");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("update_keys.rs");
    fs::write(
        output,
        format!(
            "pub const UPDATE_KEYRING_JSON: &str = {:?};\npub const UPDATE_KEYS_CONFIGURED: bool = {configured};\n",
            json
        ),
    )
    .expect("write generated updater constants");
}
