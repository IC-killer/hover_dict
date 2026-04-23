use std::io;
use std::path::PathBuf;

fn main() -> io::Result<()> {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon_path = manifest_dir.join("icon.ico");
    println!("cargo:rerun-if-changed={}", icon_path.display());

    // 使用构建目标 OS（而非 build.rs 自身的 host cfg），这样在 WSL/交叉编译到 Windows 时仍会嵌入资源。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:warning=build.rs target_os={}", target_os);
    println!(
        "cargo:warning=build.rs icon_path={} exists={}",
        icon_path.display(),
        icon_path.exists()
    );
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        let icon = icon_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "icon.ico 路径含非 UTF-8 字符")
        })?;
        res.set_icon(icon);
        // 如果资源编译器缺失/失败，这里必须直接失败，避免“编译成功但没有图标”。
        res.compile()?;

        // winres 在 windows-gnu 下默认会生成 `libresource.a` 并通过 `-lresource` 链接。
        // 但该归档里通常没有可被“引用”的符号，GNU ld 可能会把整个归档都丢弃，导致 EXE 里没有 .rsrc。
        // 这里显式把 `resource.o` 当作链接参数传入，强制把资源段并入最终可执行文件。
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        if target_env == "gnu" {
            if let Some(out_dir) = std::env::var_os("OUT_DIR") {
                let resource_o = PathBuf::from(out_dir).join("resource.o");
                if resource_o.exists() {
                    println!("cargo:warning=build.rs forcing link of {}", resource_o.display());
                    println!("cargo:rustc-link-arg={}", resource_o.display());
                } else {
                    println!(
                        "cargo:warning=build.rs expected resource.o missing at {}",
                        resource_o.display()
                    );
                }
            }
        }
    }
    Ok(())
}
