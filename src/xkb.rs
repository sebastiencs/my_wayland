use xkbcommon::xkb;

pub struct Xkb {
    ctx: xkb::Context,
    keymap: xkb::Keymap,
}

impl Xkb {
    pub fn new() -> Self {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .unwrap();

        Self { ctx, keymap }
    }
}
