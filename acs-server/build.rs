//! 构建脚本：把图标资源嵌入 `acs-server.exe`。
//!
//! 图标链路：根目录 `icon-server.png`（源图）→ 本地脚本 `scripts/make-icon.ps1`
//! （该目录不入库）→ `icons/icon-server.ico`（**已入库**，构建不需要脚本）
//! →（本脚本 + `acs-server.rc`）→ exe 内嵌图标资源。
//! 这样资源管理器、任务栏、快捷方式都能显示服务端图标，无需额外随附 ico 文件。
//!
//! 图标属「美观性」资源：若环境缺少资源编译器（rc.exe / windres），
//! embed-resource 会降级为不嵌入并打印 cargo 警告，**不会中断构建**。

fn main() {
    println!("cargo:rerun-if-changed=acs-server.rc");
    println!("cargo:rerun-if-changed=icons/icon-server.ico");

    let res = embed_resource::compile("acs-server.rc", embed_resource::NONE);
    if !matches!(res, embed_resource::CompilationResult::Ok) {
        println!("cargo:warning=acs-server 图标未能嵌入 exe（{res}）；安装包图标不受影响");
    }
    // 非 Windows 或缺少资源编译器时返回 Ok，仅真正编译失败才报错
    res.manifest_optional().unwrap();
}
