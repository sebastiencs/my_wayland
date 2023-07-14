use std::os::unix::prelude::OpenOptionsExt;

struct MyLogger {
    file: Option<std::fs::File>,
}

impl MyLogger {
    fn new() -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/tmp/wayland.log")?;

        let file = Some(file);
        // let file = None;

        redirect_to_file(file.as_ref())?;
        Ok(Self { file })
    }
}

#[cfg(not(test))]
fn redirect_to_file(file: Option<&std::fs::File>) -> std::io::Result<()> {
    use {nix::fcntl::OFlag, std::os::fd::AsRawFd};

    let Some(file) = file else {
        return Ok(())
    };

    // Redirect std{out,err} to the file
    nix::unistd::dup3(file.as_raw_fd(), 1, OFlag::O_CLOEXEC)?;
    nix::unistd::dup3(file.as_raw_fd(), 2, OFlag::O_CLOEXEC)?;
    Ok(())
}

#[cfg(test)]
fn redirect_to_file(_: Option<&std::fs::File>) -> std::io::Result<()> {
    Ok(())
}

impl log::Log for MyLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        use owo_colors::{OwoColorize, Style};

        let target = record.target();
        let target = target.strip_prefix("state_management::").unwrap_or(target);
        let target = target.on_black().bright_black();

        let level_style = Style::new();
        let level_style = match record.level() {
            log::Level::Error => level_style.red().bold(),
            log::Level::Warn => level_style.yellow(),
            log::Level::Info => level_style.green(),
            log::Level::Debug => level_style.blue(),
            log::Level::Trace => level_style.cyan(),
        };

        eprintln!(
            "[{:5}] {}{}{} {}",
            record.level().style(level_style),
            "[".on_black().bright_black(),
            target,
            "]".on_black().bright_black(),
            record.args()
        );
    }

    fn flush(&self) {}
}

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

    let logger = MyLogger::new().expect("Failed to create logger");
    log::set_boxed_logger(Box::new(logger)).unwrap();
    log::set_max_level(log::LevelFilter::Trace);

    // let mut builder = Builder::from_default_env();

    // builder
    //     .format_module_path(false)
    //     .format_target(false)
    //     .filter(None, log::LevelFilter::Trace)
    //     .init();
}
