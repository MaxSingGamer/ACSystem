// 构建脚本：把面板图标嵌入 AlphaCoinPanel.exe（图标沿用原服务端图标的视觉）
fn main() {
    println!("cargo:rerun-if-changed=panel.rc");
    println!("cargo:rerun-if-changed=icons/icon-panel.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/icon-panel.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=AlphaCoinPanel 图标未能嵌入 exe：{e}");
    }
}