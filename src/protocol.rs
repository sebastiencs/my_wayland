//! Protocol codegen for `src/protocol_generated.rs`.
//!
//! Emits, per interface: `Constants`, [`crate::object::IdToObject`], and
//! [`crate::object::ObjectWithId`] (same pairing as `IdToObject::Object` / `ObjectWithId::Id`).
//!
//! Run `cargo test regenerate_protocol_generated -- --ignored` to rewrite that file.
//! **Note:** `/usr/share/wayland/wayland.xml` (and other XML paths in `gen()`) must match
//! what this compositor implements; a newer core Wayland can add interfaces (e.g. `wl_fixes`)
//! or requests and break the build until `wayland_objects` is updated.

use std::{
    borrow::Cow,
    ffi::CString,
    fs::File,
    io::{BufWriter, Write},
    os::fd::FromRawFd,
};

use {itertools::Itertools, wayland_scanner::protocol::Interface};

use {
    heck::{ToShoutySnakeCase, ToUpperCamelCase},
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
            Some(interface) => Cow::Owned(interface_name_to_id(interface)),
            None => Cow::Borrowed("WaylandId"),
        },
        protocol::Type::NewId => match arg.interface.as_ref() {
            Some(interface) => Cow::Owned(interface_name_to_id(interface)),
            None => Cow::Borrowed("NewId"),
        },
        // protocol::Type::NewId => Cow::Owned(arg.interface.as_ref().unwrap().to_upper_camel_case()),
        protocol::Type::Array => Cow::Borrowed("Array"),
        // protocol::Type::Array => Cow::Borrowed(todo!()),
        // Fd args carry an owned reference: inbound fds are freshly received from
        // the socket's ancillary data, outbound fds are released when the message
        // is out, so the last reference closes the fd (a clipboard pipe reaches
        // EOF) or the original owner keeps it open (the shared keymap).
        protocol::Type::Fd => Cow::Borrowed("Rc<OwnedFd>"),
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

fn interface_name_to_id(interface_name: &str) -> String {
    let this_interface = interface_name.to_upper_camel_case();
    this_interface + "Id"
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
        writeln!(f, "  #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]")?;
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

fn type_to_method_name(typ: protocol::Type, interface_name: Option<&String>) -> &'static str {
    match typ {
        protocol::Type::Int => "int",
        protocol::Type::Uint => "uint",
        protocol::Type::Fixed => "fixed",
        protocol::Type::String => "string",
        protocol::Type::Object => "id",
        protocol::Type::NewId => {
            if interface_name.is_some() {
                "id"
            } else {
                "new_id_with_interface"
            }
        }
        protocol::Type::Array => "array",
        protocol::Type::Fd => "fd",
        protocol::Type::Destructor => todo!(),
    }
}

fn is_untyped_new_id(arg: &protocol::Arg) -> bool {
    arg.typ == protocol::Type::NewId && arg.interface.is_none()
}

fn write_event_writer(
    f: &mut impl Write,
    this_interface: &str,
    msgs: &[Message],
) -> std::io::Result<()> {
    writeln!(
        f,
        "  impl EventWriter for crate::wayland_objects::{}::{} {{",
        this_interface,
        this_interface.to_upper_camel_case()
    )?;
    writeln!(f, "    type Event = Event;")?;
    writeln!(f, "    fn object_id(&self) -> WaylandId {{")?;
    writeln!(f, "      use crate::object::Object;")?;
    writeln!(f, "      self.id()")?;
    writeln!(f, "    }}")?;
    writeln!(f, "    fn to_raw_event(&self, event: Event) -> RawEvent {{")?;
    writeln!(f, "      use Event::*;")?;
    writeln!(f, "      use crate::object::Object;")?;
    writeln!(f, "      match event {{")?;

    for (index, event) in msgs.iter().enumerate() {
        let name = event.name.to_upper_camel_case();
        let args = event.args.iter().map(|a| a.name.as_str()).join(",");
        writeln!(f, "        {} {{ {} }} => {{", name, args)?;
        writeln!(
            f,
            "          let mut raw = RawEvent::new(self.id(), {}, {});",
            index,
            event.args.len()
        )?;
        for arg in &event.args {
            let method = type_to_method_name(arg.typ, arg.interface.as_ref());
            writeln!(f, "          raw.{}({});", method, &arg.name)?;
        }
        writeln!(f, "        raw")?;
        writeln!(f, "        }},")?;
    }

    writeln!(f, "      }}")?;
    writeln!(f, "    }}")?;

    writeln!(f, "  }}")?;
    Ok(())
}

fn write_parser(f: &mut impl Write, this_interface: &str, msgs: &[Message]) -> std::io::Result<()> {
    writeln!(
        f,
        "  impl Parser for crate::wayland_objects::{}::{} {{",
        this_interface,
        this_interface.to_upper_camel_case()
    )?;
    writeln!(f, "    type Request = Request;")?;
    writeln!(
        f,
        "    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {{"
    )?;
    writeln!(f, "      use Request::*;")?;
    writeln!(f, "      let request = match req.request_id {{")?;

    for (index, msg) in msgs.iter().enumerate() {
        let name = msg.name.to_upper_camel_case();
        if msg.args.is_empty() {
            writeln!(f, "        {} => {},", index, name)?;
        } else {
            let untyped_new_ids = msg.args.iter().filter(|a| is_untyped_new_id(a)).count();
            if untyped_new_ids > 1 {
                panic!(
                    "protocol codegen: request `{}::{}` has multiple untyped new_id args; extend write_parser",
                    this_interface, msg.name
                );
            }
            if untyped_new_ids == 1 {
                writeln!(f, "        {} => {{", index)?;
                for arg in &msg.args {
                    if is_untyped_new_id(arg) {
                        writeln!(
                            f,
                            "          let ({}, requested_version) = req.new_id_with_interface()?;",
                            arg.name
                        )?;
                    } else {
                        let method = type_to_method_name(arg.typ, arg.interface.as_ref());
                        writeln!(f, "          let {} = req.{}()?;", &arg.name, method)?;
                    }
                }
                let fields = msg
                    .args
                    .iter()
                    .map(|a| a.name.as_str())
                    .chain(std::iter::once("requested_version"))
                    .join(", ");
                writeln!(f, "          {} {{ {} }}", name, fields)?;
                writeln!(f, "        }},")?;
            } else {
                writeln!(f, "        {} => {} {{", index, name)?;
                for arg in &msg.args {
                    let method = type_to_method_name(arg.typ, arg.interface.as_ref());
                    writeln!(f, "          {}: req.{}()?,", &arg.name, method)?;
                }
                writeln!(f, "        }},")?;
            }
        }
    }

    writeln!(f, "        _ => return Err(ParserError::InvalidRequest),")?;
    writeln!(f, "      }};")?;
    writeln!(f, "      Ok(request)")?;
    writeln!(f, "    }}")?;

    writeln!(f, "  }}")?;

    Ok(())
}

/// Emits `pub const NAME_SINCE: u32 = N` for each wire message (effective introduced version).
fn write_message_since_constants(f: &mut impl Write, msgs: &[Message]) -> std::io::Result<()> {
    for msg in msgs {
        let v = msg.effective_introduced_version();
        // ignore since = 1
        if v > 1 {
            let const_name = format!("{}_SINCE", msg.name.to_shouty_snake_case());
            writeln!(
                f,
                "  /// Minimum interface version for the `{}` message (XML `since` / `version`).",
                msg.name
            )?;
            writeln!(f, "  pub const {}: u32 = {};", const_name, v)?;
        }
    }
    Ok(())
}

fn write_message(f: &mut impl Write, name: &str, msgs: &[Message]) -> std::io::Result<()> {
    writeln!(f, "  #[derive(Debug)]")?;
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
                if is_untyped_new_id(arg) {
                    writeln!(
                        f,
                        "      /// Client-requested interface version (from `new_id` marshalling),"
                    )?;
                    writeln!(f, "      requested_version: u32,")?;
                }
            }
            writeln!(f, "    }},")?;
        }
    }
    writeln!(f, "  }}")?;
    Ok(())
}

fn write_constants(f: &mut impl Write, interface: &Interface) -> std::io::Result<()> {
    writeln!(
        f,
        "  impl Constants for crate::wayland_objects::{}::{} {{",
        interface.name,
        interface.name.to_upper_camel_case()
    )?;
    writeln!(f, "    const INTERFACE_NAME: &'static str = \"{}\";", interface.name)?;
    writeln!(f, "    const INTERFACE_VERSION: u32 = {};", interface.version)?;
    writeln!(f, "  }}")?;

    writeln!(
        f,
        "  impl crate::object::IdToObject for {} {{",
        interface_name_to_id(&interface.name),
    )?;
    writeln!(
        f,
        "    type Object = crate::wayland_objects::{}::{};",
        interface.name,
        interface.name.to_upper_camel_case()
    )?;
    writeln!(f, "  }}")?;

    writeln!(
        f,
        "  impl crate::object::ObjectWithId for crate::wayland_objects::{}::{} {{",
        interface.name,
        interface.name.to_upper_camel_case()
    )?;
    writeln!(f, "    type Id = {};", interface_name_to_id(&interface.name))?;
    writeln!(f, "  }}")?;

    Ok(())
}

pub fn gen() -> std::io::Result<()> {
    // dnf install wayland-protocols-devel wlr-protocols-devel hyprland-protocols-devel
    let paths = [
        "/usr/share/wayland/wayland.xml",
        "/usr/share/wayland-protocols/stable/linux-dmabuf/linux-dmabuf-v1.xml",
        "/usr/share/wayland-protocols/stable/tablet/tablet-v2.xml",
        "/usr/share/wayland-protocols/stable/presentation-time/presentation-time.xml",
        "/usr/share/wayland-protocols/stable/viewporter/viewporter.xml",
        "/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml",
        // "/usr/share/wayland-protocols/staging/alpha-modifier/alpha-modifier-v1.xml",
        // "/usr/share/wayland-protocols/staging/color-management/color-management-v1.xml",
        // "/usr/share/wayland-protocols/staging/color-representation/color-representation-v1.xml",
        // "/usr/share/wayland-protocols/staging/commit-timing/commit-timing-v1.xml",
        "/usr/share/wayland-protocols/staging/content-type/content-type-v1.xml",
        "/usr/share/wayland-protocols/staging/drm-lease/drm-lease-v1.xml",
        // "/usr/share/wayland-protocols/staging/ext-background-effect/ext-background-effect-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-data-control/ext-data-control-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-idle-notify/ext-idle-notify-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-image-capture-source/ext-image-capture-source-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-session-lock/ext-session-lock-v1.xml",
        // "/usr/share/wayland-protocols/staging/ext-transient-seat/ext-transient-seat-v1.xml",
        // "/usr/share/wayland-protocols/staging/fifo/fifo-v1.xml",
        "/usr/share/wayland-protocols/staging/fractional-scale/fractional-scale-v1.xml",
        // "/usr/share/wayland-protocols/staging/pointer-warp/pointer-warp-v1.xml",
        // "/usr/share/wayland-protocols/staging/security-context/security-context-v1.xml",
        "/usr/share/wayland-protocols/staging/single-pixel-buffer/single-pixel-buffer-v1.xml",
        "/usr/share/wayland-protocols/staging/tearing-control/tearing-control-v1.xml",
        "/usr/share/wayland-protocols/staging/xdg-activation/xdg-activation-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-dialog/xdg-dialog-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-session-management/xdg-session-management-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-system-bell/xdg-system-bell-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-toplevel-icon/xdg-toplevel-icon-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-toplevel-tag/xdg-toplevel-tag-v1.xml",
        "/usr/share/wayland-protocols/staging/xwayland-shell/xwayland-shell-v1.xml",
        "/usr/share/wayland-protocols/staging/cursor-shape/cursor-shape-v1.xml",
        "/usr/share/wayland-protocols/staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml",
        // "/usr/share/wayland-protocols/unstable/fullscreen-shell/fullscreen-shell-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/idle-inhibit/idle-inhibit-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/input-method/input-method-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/input-timestamps/input-timestamps-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/keyboard-shortcuts-inhibit/keyboard-shortcuts-inhibit-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/linux-explicit-synchronization/linux-explicit-synchronization-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/pointer-constraints/pointer-constraints-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/pointer-gestures/pointer-gestures-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/primary-selection/primary-selection-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/relative-pointer/relative-pointer-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/tablet/tablet-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/text-input/text-input-unstable-v1.xml",
        "/usr/share/wayland-protocols/unstable/text-input/text-input-unstable-v3.xml",
        "/usr/share/wayland-protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-foreign/xdg-foreign-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-foreign/xdg-foreign-unstable-v2.xml",
        "/usr/share/wayland-protocols/unstable/xdg-output/xdg-output-unstable-v1.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-shell/xdg-shell-unstable-v5.xml",
        // "/usr/share/wayland-protocols/unstable/xdg-shell/xdg-shell-unstable-v6.xml",
        // "/usr/share/wayland-protocols/unstable/xwayland-keyboard-grab/xwayland-keyboard-grab-unstable-v1.xml",
        // "/usr/share/wayland-protocols/staging/xdg-toplevel-drag/xdg-toplevel-drag-v1.xml",
        "/usr/share/wayland-protocols/staging/ext-workspace/ext-workspace-v1.xml",

        "/usr/share/wlr-protocols/unstable/wlr-data-control-unstable-v1.xml",
        "/usr/share/wlr-protocols/unstable/wlr-foreign-toplevel-management-unstable-v1.xml",
        // "/usr/share/wlr-protocols/unstable/wlr-input-inhibitor-unstable-v1.xml",
        // "/usr/share/wlr-protocols/unstable/wlr-output-management-unstable-v1.xml",
        "/usr/share/wlr-protocols/unstable/wlr-screencopy-unstable-v1.xml",
        // "/usr/share/wlr-protocols/unstable/wlr-export-dmabuf-unstable-v1.xml",
        "/usr/share/wlr-protocols/unstable/wlr-gamma-control-unstable-v1.xml",
        "/usr/share/wlr-protocols/unstable/wlr-layer-shell-unstable-v1.xml",
        // "/usr/share/wlr-protocols/unstable/wlr-output-power-management-unstable-v1.xml",
        // "/usr/share/wlr-protocols/unstable/wlr-virtual-pointer-unstable-v1.xml",

        concat!(env!("CARGO_MANIFEST_DIR"), "/protocols/input-method-unstable-v2.xml"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/protocols/virtual-keyboard-unstable-v1.xml"),

        "/home/sebastien/github/hyprland-protocols/protocols/hyprland-ctm-control-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-global-shortcuts-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-lock-notify-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-toplevel-export-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-focus-grab-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-input-capture-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-surface-v1.xml",
        // "/home/sebastien/github/hyprland-protocols/protocols/hyprland-toplevel-mapping-v1.xml",
    ];

    let out_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/protocol_generated.rs");
    let file = File::create(&out_path)?;
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

    let derive_str =
        "#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]";

    writeln!(f, "#![allow(dead_code, unused_imports, unused_mut, clippy::let_and_return)]")?;
    writeln!(f, "#![allow(clippy::enum_variant_names)]\n")?;
    writeln!(f, "use std::os::unix::io::RawFd;")?;
    writeln!(f, "use std::os::fd::OwnedFd;")?;
    writeln!(f, "use std::rc::Rc;")?;
    writeln!(
        f,
        "use crate::{{clients::{{RawRequest, RawEvent, EventWriter}}, object::{{Parser, ParserError}}}};\n"
    )?;
    writeln!(f, "{}", derive_str)?;
    writeln!(f, "pub struct WaylandId(u32);")?;
    writeln!(f, "{}", derive_str)?;
    writeln!(f, "pub struct NewId(u32);")?;

    let derive_str =
        "#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into, derive_more::Deref)]";
    writeln!(f, "{}", derive_str)?;
    writeln!(f, "pub struct Array(pub Vec<u32>);\n")?;

    writeln!(f, "wayland_id! {{")?;
    for protocol in &protocols {
        for interface in &protocol.interfaces {
            let name = interface_name_to_id(&interface.name);
            writeln!(f, "  {},", name)?;
        }
    }
    writeln!(f, "}}\n")?;

    writeln!(f, "pub trait Constants {{")?;
    writeln!(f, "  const INTERFACE_NAME: &'static str;")?;
    writeln!(f, "  const INTERFACE_VERSION: u32;")?;
    writeln!(f, "}}\n")?;

    for protocol in &protocols {
        for interface in &protocol.interfaces {
            let this_interface = &interface.name;

            write_description(f, "/// ", interface.description.as_ref())?;
            writeln!(f, "pub mod {} {{", interface.name)?;
            writeln!(f, "  use super::*;")?;
            // writeln!(f, "  const INTERFACE_NAME: &'static str = \"{}\";", interface.name)?;
            // writeln!(f, "  const INTERFACE_VERSION: u32 = {};", interface.version)?;

            write_constants(f, interface)?;
            if !interface.requests.is_empty() {
                write_message_since_constants(f, &interface.requests)?;
                write_message(f, "Request", &interface.requests)?;
                write_parser(f, this_interface, &interface.requests)?;
            }
            if !interface.events.is_empty() {
                write_message_since_constants(f, &interface.events)?;
                write_message(f, "Event", &interface.events)?;
                write_event_writer(f, this_interface, &interface.events)?;
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

#[cfg(test)]
mod protocol_gen_tests {
    #[test]
    #[ignore = "rewrites src/protocol_generated.rs; run: cargo test regenerate_protocol_generated -- --ignored"]
    fn regenerate_protocol_generated() {
        super::gen().unwrap();
    }
}
