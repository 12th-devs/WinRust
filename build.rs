fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=payload");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
}
