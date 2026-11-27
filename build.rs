use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "NanoPing");
        res.set("FileDescription", "NanoPing");
        res.set("OriginalFilename", "NanoPing.exe");
        res.set("CompanyName", "NetForgeDev");

        let icon_path = "assets/latency.ico";
        if Path::new(icon_path).exists() {
            res.set_icon(icon_path);
            println!("cargo:rerun-if-changed={icon_path}");
        }

        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resources: {e}");
        }
    }
}