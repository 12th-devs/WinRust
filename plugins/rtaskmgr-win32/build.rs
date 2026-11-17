fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=app.manifest");
    // иконка + версия + манифест (Common Controls v6) одним ресурсом
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
}
