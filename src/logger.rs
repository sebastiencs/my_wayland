use env_logger::Builder;

pub fn install() {
    let mut builder = Builder::from_default_env();

    builder
        .format_module_path(false)
        .format_target(false)
        .filter(None, log::LevelFilter::Debug)
        .init();
}
