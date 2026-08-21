//! 平台/系统工具（对应源项目 Utils/SystemHelper.cs、Utils/PathHelper.cs、Utils/OfflineUuidHelper.cs）
//! SystemHelper：OS / 架构探测；PathHelper：Minecraft 路径约定；OfflineUuidHelper：离线 UUID v3

use std::path::{Path, PathBuf};

use crate::models::version_metadata::OsRequirement;

// ── SystemHelper（系统探测）──────────────────────────

/// 判断当前系统是否满足指定的操作系统要求（对应 SystemHelper.IsOsMatch）
pub fn is_os_match(os: &OsRequirement) -> bool {
    // `name` may be absent (e.g. an arch-only rule `{"arch":"x86"}`) in newer
    // versions; when missing, the OS name requirement is treated as satisfied.
    if let Some(name) = &os.name {
        if name != get_current_os_name() {
            return false;
        }
    }

    // C#: !string.IsNullOrEmpty(os.Version) && !Environment.OSVersion.VersionString.Contains(os.Version)
    // ⚠️ UNMAPPED: std 无法获取 OS 版本字符串（.NET 的 VersionString 形如 "Microsoft Windows NT 10.0.22631.0"）。
    //    需要 os_info crate 或平台专用 API；目前无法获取时按"不匹配"处理。
    if let Some(version) = &os.version {
        if !version.is_empty() {
            let Some(version_string) = current_os_version_string() else {
                return false;
            };
            if !version_string.contains(version.as_str()) {
                return false;
            }
        }
    }

    if let Some(arch) = &os.arch {
        if !arch.is_empty() && arch != get_current_arch() {
            return false;
        }
    }

    true
}

/// 获取当前操作系统名称（对应 SystemHelper.GetCurrentOsName）
/// 返回 "windows" / "linux" / "osx"（macOS）/ "unknown"
pub fn get_current_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        "linux"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "unknown"
    }
}

/// 获取当前操作系统版本字符串（.NET Environment.OSVersion.VersionString 的对应物）
/// ⚠️ UNMAPPED: 标准库不提供 OS 版本信息。需引入 os_info crate 或平台专用 API（uname / sysctl）。
/// 目前恒返回 None，is_os_match 在存在版本要求时将视为不匹配。
fn current_os_version_string() -> Option<String> {
    None
}

/// 当前架构位数（对应 SystemHelper.GetCurrentArch）：64 位 OS 返回 "64"，否则 "32"
/// 说明：C# 依据 Environment.Is64BitOperatingSystem（运行环境），Rust 依据编译目标架构
/// std::env::consts::ARCH（AOT 静态编译下通常与运行环境一致，除 WOW64 等极端场景）
pub fn get_current_arch() -> &'static str {
    if std::env::consts::ARCH.ends_with("64") {
        "64"
    } else {
        "32"
    }
}

/// 获取环境变量路径分隔符（对应 SystemHelper.GetSeparator）：Windows ";"，其余 ":"
pub fn get_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// 获取当前架构的运行时名称（对应 SystemHelper.GetArch / RuntimeInformation.OSArchitecture 小写）
/// 返回 "x64" / "arm64" / "x86" / "arm" / "wasm" / "s390x" / "loongarch64" 等
/// 说明：Rust std::env::consts::ARCH 与 .NET 命名不同（x86_64→x64、aarch64→arm64、wasm32→wasm），此处显式映射
pub fn get_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "x86",
        "x86_64" => "x64",
        "arm" => "arm",
        "aarch64" => "arm64",
        "wasm32" | "wasm64" => "wasm",
        "s390x" => "s390x",
        "loongarch64" => "loongarch64",
        other => other,
    }
}

// ── PathHelper（Minecraft 路径约定）──────────────────

/// 获取 Minecraft 根目录（对应 PathHelper.GetMinecraftPath）：应用数据目录 + ".minecraft"
/// C# 取 Environment.SpecialFolder.ApplicationData：
///   Windows → %APPDATA%，macOS → ~/Library/Application Support，Linux → $XDG_CONFIG_HOME 或 ~/.config
/// 基础目录无法解析时返回 None（C# 对应返回空字符串）
pub fn get_minecraft_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join(".minecraft"))
}

/// 获取版本目录（对应 PathHelper.GetVersionPath）：gameRoot/versions/versionId
pub fn get_version_path(game_root: impl AsRef<Path>, version_id: &str) -> PathBuf {
    game_root.as_ref().join("versions").join(version_id)
}

/// 获取 libraries 目录（对应 PathHelper.GetLibrariesPath）：gameRoot/libraries
pub fn get_libraries_path(game_root: impl AsRef<Path>) -> PathBuf {
    game_root.as_ref().join("libraries")
}

/// 获取 assets 目录（对应 PathHelper.GetAssetsPath）：gameRoot/assets
pub fn get_assets_path(game_root: impl AsRef<Path>) -> PathBuf {
    game_root.as_ref().join("assets")
}

/// 路径规范化（对应 PathHelper.NormalizePath）：转为绝对路径并将分隔符统一为 '/'
/// C# Path.GetFullPath 语义：相对路径基于当前工作目录解析，词法折叠 "." / ".."，
/// 不要求路径存在（std::fs::canonicalize 要求存在且解析符号链接，语义不同，此处不采用）。
/// 与 C# 的差异：不保留末尾分隔符。
pub fn normalize_path(path: &str) -> String {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => p.to_path_buf(),
        }
    };
    normalize_lexically(&full)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 词法折叠路径中的 "." 与 ".." 段（不访问文件系统）
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // 仅当末段是普通段时才回退，否则钳制在根（匹配 GetFullPath 行为）
                if matches!(
                    out.components().next_back(),
                    Some(std::path::Component::Normal(_))
                ) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ── OfflineUuidHelper（离线 UUID）────────────────────

/// 生成离线 UUID（对应 OfflineUuidHelper.GenerateUuid）
/// 语义：MD5("OfflinePlayer:{name}") → 32 位小写十六进制，再改写版本位（0x30 = v3）与变体位（0x80）
/// ⚠️ 需要依赖: md-5 = "0.10"（RustCrypto；Cargo.toml 尚未引入）
/// 说明：C# 对空字符串返回空串；未使用 uuid crate，按原始字节逻辑移植
pub fn generate_uuid(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }

    use md5::{Digest, Md5};
    let digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest);

    // C#: bit6 = byte(hex[12..14] & 0x0F | 0x30)，bit8 = byte(hex[16..18] & 0x3F | 0x80)
    bytes[6] = bytes[6] & 0x0F | 0x30; // 版本 nibble → v3
    bytes[8] = bytes[8] & 0x3F | 0x80; // 变体位 → RFC 4122

    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}
