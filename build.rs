fn main() {
    println!("cargo:rerun-if-changed=assets/flux.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Ressource n°1 : icône de l'exe, reprise par la fenêtre (main.rs).
        res.set_icon_with_id("assets/flux.ico", "1");
        res.set("FileDescription", "Flux");
        res.set("ProductName", "Flux");
        res.compile().expect("compilation des ressources Windows");
    }
}
