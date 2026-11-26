fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Unkn0WnTorrent");
        res.set("FileDescription", "Unkn0WnTorrent - fast torrent client");
        res.set("LegalCopyright", "MIT");
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {err}");
        }
    }
}
