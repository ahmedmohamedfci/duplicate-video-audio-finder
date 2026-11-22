fn main() {
    let public = std::path::Path::new("../public");
    if !public.join("index.html").exists() {
        panic!("public/index.html missing — UI assets required for embed");
    }
    tauri_build::build()
}
