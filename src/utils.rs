pub fn is_x11() -> bool {
    std::env::var("DISPLAY").is_ok()
}
