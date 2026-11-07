//! On Windows, embeds the Pro Wave icon and product name in `prowave.exe`, so Explorer,
//! shortcuts and taskbar pins show the logo. Other targets set their icons at run time
//! (`ViewportBuilder::with_icon`) or through their package files (dist/linux, dist/macos).

fn main() {
    windows_icon();
}

fn windows_icon() {
    println!("cargo:rerun-if-changed=assets/prowave.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/prowave.ico")
        .set("ProductName", "Pro Wave")
        .set("FileDescription", "Pro Wave");
    // An icon is not worth failing a build over (e.g. no Windows SDK resource compiler).
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
