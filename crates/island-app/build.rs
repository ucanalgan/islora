fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=../../assets");
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embedding the icons failed");
}
