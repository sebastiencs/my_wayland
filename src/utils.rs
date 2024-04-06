pub fn is_x11() -> bool {
    std::env::var("DISPLAY").is_ok()
}

pub fn ptr_or_null_mut<T>(v: &mut [T]) -> *mut T {
    if v.is_empty() {
        std::ptr::null_mut()
    } else {
        v.as_mut_ptr()
    }
}

pub fn ptr_or_null<T>(v: &[T]) -> *const T {
    if v.is_empty() {
        std::ptr::null()
    } else {
        v.as_ptr()
    }
}

pub fn strip_type_name(s: &str) -> &str {
    let index = s.rfind("::").map(|i| i + 2).unwrap_or(0);
    s.get(index..).unwrap()
}
