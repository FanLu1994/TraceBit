fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=tracebit.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        embed_resource::compile("tracebit.rc", embed_resource::NONE)
            .manifest_required()
            .expect("无法嵌入 TraceBit Windows 图标资源");
    }
}
