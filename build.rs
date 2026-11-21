fn main() {
    slint_build::compile("ui/app.slint").expect("ui/app.slint must compile");
    println!("cargo:rerun-if-changed=ui");
}
