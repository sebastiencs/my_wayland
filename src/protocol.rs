use std::{
    borrow::Cow,
    ffi::CString,
    fs::File,
    io::{BufWriter, Write},
    os::fd::FromRawFd,
};

use {
    heck::ToUpperCamelCase,
    libc::memfd_create,
    wayland_scanner::protocol::{self, Enum, Message},
};

fn temp_file() -> std::fs::File {
    let name = CString::new("tmp_file").unwrap();
    let result = unsafe { memfd_create(name.as_ptr(), 0) };
    assert_ne!(result, -1);

    unsafe { File::from_raw_fd(result) }
}

fn type_to_name(arg: &protocol::Arg) -> Cow<'static, str> {
    match arg.typ {
        protocol::Type::Int => Cow::Borrowed("i32"),
        protocol::Type::Uint => Cow::Borrowed("u32"),
        protocol::Type::Fixed => Cow::Borrowed("f32"),
        protocol::Type::String => Cow::Borrowed("String"),
        // protocol::Type::Object => Cow::Borrowed(todo!()),
        protocol::Type::Object => match arg.interface.as_ref() {
            Some(interface) => Cow::Owned(interface.to_upper_camel_case()),
            None => Cow::Borrowed("ObjectId"),
        },
        protocol::Type::NewId => match arg.interface.as_ref() {
            Some(interface) => Cow::Owned(interface.to_upper_camel_case()),
            None => Cow::Borrowed("NewId"),
        },
        // protocol::Type::NewId => Cow::Owned(arg.interface.as_ref().unwrap().to_upper_camel_case()),
        protocol::Type::Array => Cow::Borrowed("Array"),
        // protocol::Type::Array => Cow::Borrowed(todo!()),
        protocol::Type::Fd => Cow::Borrowed("RawFd"),
        protocol::Type::Destructor => todo!(),
    }
}

fn format_long_description(s: &str) -> Vec<&str> {
    s.split('\n').map(str::trim).collect::<Vec<_>>()
}

fn first_char_to_uppercase(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let mut s = s.chars();
    s.next().unwrap().to_uppercase().chain(s).collect()
}

fn write_description(
    f: &mut impl Write,
    prefix: &str,
    description: Option<&(String, String)>,
) -> std::io::Result<()> {
    if let Some((short, long)) = description.as_ref() {
        let short = first_char_to_uppercase(short);
        writeln!(f, "{}{}", prefix, short)?;
        let long = format_long_description(long);
        if !long.is_empty() {
            writeln!(f, "{}", prefix)?;
        }
        for line in long {
            writeln!(f, "{}{}", prefix, line)?;
        }
    }
    Ok(())
}

fn write_enums(f: &mut impl Write, enums: &[Enum]) -> std::io::Result<()> {
    for e in enums {
        write_description(f, "  /// ", e.description.as_ref())?;
        writeln!(f, "  pub enum {} {{", e.name.to_upper_camel_case())?;
        for entry in &e.entries {
            if let Some(summary) = entry.summary.as_ref() {
                writeln!(f, "    /// {},", first_char_to_uppercase(summary))?;
            };
            write_description(f, "      /// ", entry.description.as_ref())?;

            let name = if entry.name.chars().next().unwrap().is_numeric() {
                // Variant starting with numbers are not valid Rust
                "_".to_string() + &entry.name.to_upper_camel_case()
            } else {
                entry.name.to_upper_camel_case()
            };

            writeln!(f, "    {} = {},", name, entry.value)?;
        }
        writeln!(f, "  }}")?;
    }
    Ok(())
}

fn write_message(
    f: &mut impl Write,
    name: &str,
    this_interface: &str,
    msgs: &[Message],
) -> std::io::Result<()> {
    writeln!(f, "  pub enum {} {{", name)?;
    for req in msgs {
        write_description(f, "    /// ", req.description.as_ref())?;
        let name = req.name.to_upper_camel_case();

        if req.args.is_empty() {
            writeln!(f, "    {},", name)?;
        } else {
            writeln!(f, "    {} {{ ", name)?;
            // writeln!(f, "    this_id: {},", this_interface)?;
            for arg in &req.args {
                if let Some(summary) = arg.summary.as_ref() {
                    writeln!(f, "      /// {},", first_char_to_uppercase(summary))?;
                };
                write_description(f, "      /// ", arg.description.as_ref())?;
                writeln!(f, "      {}: {},", arg.name, type_to_name(arg))?;
            }
            writeln!(f, "    }},")?;
        }
    }
    writeln!(f, "  }}")?;
    Ok(())
}

pub fn gen() -> std::io::Result<()> {
    let paths = [
        "/usr/share/wayland/wayland.xml",
        "/usr/share/wayland-protocols/stable/presentation-time/presentation-time.xml",
        "/usr/share/wayland-protocols/stable/viewporter/viewporter.xml",
        "/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml",
        "/usr/share/wayland-protocols/staging/content-type/content-type-v1.xml",
        "/usr/share/wayland-protocols/staging/drm-lease/drm-lease-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-idle-notify/ext-idle-notify-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-session-lock/ext-session-lock-v1.xml",
        "/usr/share/wayland-protocols/staging/fractional-scale/fractional-scale-v1.xml",
        "/usr/share/wayland-protocols/staging/single-pixel-buffer/single-pixel-buffer-v1.xml",
        "/usr/share/wayland-protocols/staging/tearing-control/tearing-control-v1.xml",
        "/usr/share/wayland-protocols/staging/xdg-activation/xdg-activation-v1.xml",
        "/usr/share/wayland-protocols/staging/xwayland-shell/xwayland-shell-v1.xml",
        // "/usr/share/wayland-protocols/unstable/fullscreen-shell/fullscreen-shell-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/idle-inhibit/idle-inhibit-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/input-method/input-method-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/input-timestamps/input-timestamps-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/keyboard-shortcuts-inhibit/keyboard-shortcuts-inhibit-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/linux-dmabuf/linux-dmabuf-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/linux-explicit-synchronization/linux-explicit-synchronization-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/pointer-constraints/pointer-constraints-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/pointer-gestures/pointer-gestures-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/primary-selection/primary-selection-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/relative-pointer/relative-pointer-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/tablet/tablet-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/tablet/tablet-unstable-v2.xml",
        // "/usr/share/wayland-protocols/unstable/text-input/text-input-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/text-input/text-input-unstable-v3.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-foreign/xdg-foreign-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-foreign/xdg-foreign-unstable-v2.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-output/xdg-output-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-shell/xdg-shell-unstable-v5.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-shell/xdg-shell-unstable-v6.xml",
        // "/usr/share/wayland-protocols/unstable/xwayland-keyboard-grab/xwayland-keyboard-grab-unstable-v1.xml",
    ];

    let file = File::create("/home/sebastien/tmp/testaa/src/protocol.rs")?;
    let mut file = BufWriter::with_capacity(32 * 1024 * 1024, file);

    // let mut file = temp_file();
    let f = &mut file;

    let protocols = paths
        .iter()
        .map(File::open)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(wayland_scanner::parse::parse)
        .collect::<Vec<_>>();

    writeln!(f, "#![allow(dead_code, unused_imports)]")?;
    writeln!(f, "#![allow(clippy::enum_variant_names)]\n")?;
    writeln!(f, "use std::os::unix::io::RawFd;\n")?;
    writeln!(f, "pub struct ObjectId;")?;
    writeln!(f, "pub struct NewId;")?;
    writeln!(f, "pub struct Array;")?;

    for protocol in &protocols {
        for interface in &protocol.interfaces {
            let this_interface = interface.name.to_upper_camel_case();
            writeln!(f, "pub struct {};", this_interface)?;
        }
    }
    writeln!(f)?;

    for protocol in &protocols {
        for interface in &protocol.interfaces {
            dbg!(interface);
            let this_interface = interface.name.to_upper_camel_case();

            write_description(f, "/// ", interface.description.as_ref())?;
            writeln!(f, "pub mod {} {{", interface.name)?;
            writeln!(f, "  use super::*;")?;

            if !interface.requests.is_empty() {
                write_message(f, "Request", &this_interface, &interface.requests)?;
            }
            if !interface.events.is_empty() {
                write_message(f, "Event", &this_interface, &interface.events)?;
            }
            write_enums(f, &interface.enums)?;

            writeln!(f, "}}\n")?;
        }
    }

    f.flush()?;
    // f.seek(std::io::SeekFrom::Start(0))?;

    // let content = std::io::read_to_string(f)?;
    // println!("content=\n{}", content);

    Ok(())
}
