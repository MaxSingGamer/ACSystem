// 构建脚本：把面板图标嵌入 AlphaCoinServerPanel.exe
fn main() {
    println!("cargo:rerun-if-changed=panel.rc");
    println!("cargo:rerun-if-changed=icons/icon-panel.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/icon-panel.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=AlphaCoinServerPanel 图标未能嵌入 exe：{e}");
    }
}