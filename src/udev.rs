
pub fn init() -> Result<(), std::io::Error> {
    let udev = udev::Udev::new()?;

    let monitor = udev::MonitorBuilder::new()?
        .match_subsystem("input")?
        .match_subsystem("drm")?
        .listen()?;

    for e in monitor.iter() {
        eprintln!("EVENT={:?}", e);
    }

    Ok(())
}
