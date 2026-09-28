//! Gives the Windows programs dowse's icon (packaging/windows).

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/dowse.rc");
    println!("cargo:rerun-if-changed=packaging/windows/dowse.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile(
            "packaging/windows/dowse.rc",
            embed_resource::ParamsIncludeDirs(["packaging/windows"]),
        )
        .manifest_optional()
        .unwrap();
    }
}
