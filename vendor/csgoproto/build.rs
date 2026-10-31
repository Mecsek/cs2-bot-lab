//! Vendored from LaihoE/demoparser@314a253 (src/csgoproto). Upstream clones the latest
//! SteamDatabase/GameTracking-CS2 protos at build time; here they are pinned in `protos/`
//! (GameTracking-CS2@10f3693, 2026-09-23) so builds are reproducible and offline.
//! Requires `protoc` (protobuf-compiler).

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=protos");
    let protos = [
        "steammessages", "gcsdk_gcmessages", "demo", "cstrike15_gcmessages", "cstrike15_usermessages",
        "usermessages", "networkbasetypes", "engine_gcmessages", "netmessages", "network_connection",
        "cs_usercmd", "usercmd", "gameevents", "cs_gameevents",
    ]
    .map(|p| format!("protos/{p}.proto"));
    prost_build::Config::new()
        .format(false)
        .out_dir("src")
        .default_package_filename("protobuf")
        .bytes(["."])
        .enum_attribute(".", "#[derive(::strum::EnumIter)]")
        .compile_protos(&protos, &["protos/"])
}
