use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let shader_dir = Path::new("src/render/vulkan/shaders");
    println!("cargo:rerun-if-changed=src/render/vulkan/shaders");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let shaders = shader_sources(shader_dir);
    let generated =
        if shaders.is_empty() { String::new() } else { compile_shaders(&shaders, &out_dir) };
    fs::write(out_dir.join("vulkan_shaders.rs"), generated).expect("write vulkan shader module");
}

fn shader_sources(dir: &Path) -> Vec<(PathBuf, shaderc_dyn::ShaderKind)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut shaders = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = match path.extension().and_then(|extension| extension.to_str()) {
            Some("vert") => shaderc_dyn::ShaderKind::Vertex,
            Some("frag") => shaderc_dyn::ShaderKind::Fragment,
            Some("comp") => shaderc_dyn::ShaderKind::Compute,
            _ => continue,
        };
        shaders.push((path, kind));
    }
    shaders.sort_by(|left, right| left.0.cmp(&right.0));
    shaders
}

fn compile_shaders(shaders: &[(PathBuf, shaderc_dyn::ShaderKind)], out_dir: &Path) -> String {
    // shaderc-dyn opens libshaderc_shared.so. The runtime package on this
    // system ships the versioned soname and leaves the unversioned name to
    // the development package, so point at the library that is installed.
    if env::var_os("SHADERC_LIB_PATH").is_none() {
        let candidates = [
            "/lib64/libshaderc_shared.so",
            "/usr/lib64/libshaderc_shared.so",
            "/lib/libshaderc_shared.so",
            "/lib64/libshaderc_shared.so.1",
            "/usr/lib64/libshaderc_shared.so.1",
            "/lib/libshaderc_shared.so.1",
        ];
        if let Some(path) = candidates.iter().find(|path| Path::new(path).exists()) {
            env::set_var("SHADERC_LIB_PATH", path);
        }
    }
    let compiler = shaderc_dyn::Compiler::new().expect("load libshaderc_shared");
    let mut options = shaderc_dyn::CompileOptions::new().expect("shaderc options");
    options
        .set_target_env(shaderc_dyn::TargetEnv::Vulkan, shaderc_dyn::EnvVersion::Vulkan1_2 as u32);
    options.set_target_spirv(shaderc_dyn::SpirvVersion::V1_5);
    let mut generated = String::new();
    for (path, kind) in shaders {
        let source = fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("read {}: {error}", path.display());
        });
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shader");
        let spirv = compiler
            .compile_into_spirv(&source, *kind, name, "main", Some(&options))
            .unwrap_or_else(|error| panic!("compile {name}: {error}"));
        let spirv_name = format!("{name}.spv");
        fs::write(out_dir.join(&spirv_name), spirv.as_binary_u8())
            .unwrap_or_else(|error| panic!("write {spirv_name}: {error}"));
        let ident = spirv_ident(name);
        generated.push_str(&format!(
            "pub static {ident}: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{spirv_name}\"));\n"
        ));
    }
    generated
}

fn spirv_ident(file_name: &str) -> String {
    let mut ident = String::new();
    for character in file_name.chars() {
        if character.is_ascii_alphanumeric() {
            ident.push(character.to_ascii_uppercase());
        } else if !ident.ends_with('_') {
            ident.push('_');
        }
    }
    ident.trim_matches('_').to_string()
}
