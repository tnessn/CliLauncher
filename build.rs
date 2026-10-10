fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/logo.ico")
            .set("ProductName", "CLI Launcher")
            .set("FileDescription", "CLI project launcher")
            .set("OriginalFilename", "CliLauncher.exe");
        resource
            .compile()
            .expect("failed to compile Windows resources");
    }
}
