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
