use env_logger::Builder;

#[cfg(test)]
fn is_initialized() -> bool {
    use std::sync::atomic::AtomicBool;

    static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

    IS_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(not(test))]
fn is_initialized() -> bool {
    false
}

pub fn install() {
    if is_initialized() {
        return;
    }

    let mut builder = Builder::from_default_env();

    builder
        .format_module_path(false)
        .format_target(false)
        .filter(None, log::LevelFilter::Trace)
        .init();
}
