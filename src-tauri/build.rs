fn main() {
    // 集成测试（tests/*.rs）会用到 mock 运行时，它把 tauri 的 app 代码单元整个
    // 拖进测试 exe，连带 muda 的 TaskDialogIndirect（comctl32 v6 独有入口）。
    // 测试 exe 没有 tauri-build 的资源嵌入，加载器落到 comctl32 v5 就会
    // STATUS_ENTRYPOINT_NOT_FOUND——给每个测试 exe 嵌一份声明 v6 的 manifest。
    if std::env::var("CARGO_CFG_WINDOWS").is_ok()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")
            + "\\tests-comctl32.manifest";
        println!("cargo:rerun-if-changed=tests-comctl32.manifest");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{manifest}");
    }

    tauri_build::build()
}
