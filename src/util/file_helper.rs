//! 文件工具（对应源项目 Utils/FileHelper.cs）

use std::fs;
use std::io::Read;
use std::path::Path;

/// 校验文件 SHA1 哈希（对应 FileHelper.ValidateFileHash）
/// 文件不存在或期望哈希为空 → false；计算文件 SHA1（小写十六进制）并与期望值忽略大小写比较
/// ⚠️ 需要依赖: sha1 = "0.10"（RustCrypto；Cargo.toml 尚未引入）
/// 说明：C# 在读取异常时向上抛出异常，Rust 版本将 IO 错误视为校验失败返回 false
pub fn validate_file_hash(file_path: &str, expected_hash: &str) -> bool {
    if expected_hash.is_empty() || !Path::new(file_path).is_file() {
        return false;
    }

    let mut file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return false;
    }

    use sha1::{Digest, Sha1};
    let digest = Sha1::digest(&buf);
    let actual = format!("{digest:x}"); // 小写十六进制（对应 Convert.ToHexString().ToLower()）
    actual.eq_ignore_ascii_case(expected_hash)
}

/// 将文件系统路径字符串的目录分隔符统一为平台分隔符（对应
/// 后端 `install_service.rs::normalize_sep`、`launch::process.rs` 的既有先例）。
///
/// Windows 上 `canonicalize()` 产生的 verbatim 路径（`\\?\` 前缀）中 `/` 不再是
/// 路径分隔符而是普通字符，安装器经 `path_combine` 拼接 Maven 坐标路径时保留的 `/`
/// （如 `libraries\net/minecraftforge/...jar`）会令 `create_dir_all`/`rename`/
/// `std::fs::write` 报 `ERROR_INVALID_NAME (os error 123)`。为规避 verbatim 路径的
/// 特殊语义，且 Windows 上 `/` 与 `\` 等价（verbatim 除外），统一替换为 `\` 无害。
/// 非 Windows（且非 verbatim）平台上无操作。
///
/// ⚠️ 此函数仅用于**本地文件系统路径**，不可用于 URL 构造（URL 必须保留 `/`）。
pub fn normalize_separators(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

/// 格式化目录路径（对应 FileHelper.FormatDirPath）：含空格时用双引号包裹（供命令行参数使用）
pub fn format_dir_path(path: &str) -> String {
    if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// 解压 zip 到目标目录（对应 FileHelper.Unzip）
/// 压缩包不存在 → false；自动创建目标目录；覆盖已有文件（对应 ZipFile.ExtractToDirectory 的 overwriteFiles: true）
/// ⚠️ 需要依赖: zip = "2"（zip crate；Cargo.toml 尚未引入）
/// 说明：C# Trace.WriteLine 记录失败日志，Rust 使用 eprintln! 输出到 stderr
pub fn unzip(zip_file_path: &str, target_dir: &str) -> bool {
    if !Path::new(zip_file_path).is_file() {
        return false; // 压缩包不存在
    }

    // 确保目标目录存在
    if let Err(e) = fs::create_dir_all(target_dir) {
        eprintln!("解压失败：{e}");
        return false;
    }

    match extract_zip(zip_file_path, target_dir) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("解压失败：{e}");
            false
        }
    }
}

/// 逐项解压 zip（覆盖已存在文件）
fn extract_zip(zip_file_path: &str, target_dir: &str) -> zip::result::ZipResult<()> {
    let file = fs::File::open(zip_file_path).map_err(zip::result::ZipError::Io)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let out_path = Path::new(target_dir).join(entry.mangled_name());

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(zip::result::ZipError::Io)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(zip::result::ZipError::Io)?;
            }
            let mut out = fs::File::create(&out_path).map_err(zip::result::ZipError::Io)?;
            std::io::copy(&mut entry, &mut out).map_err(zip::result::ZipError::Io)?;
        }
    }
    Ok(())
}

/// 删除指定目录中除特定后缀外的所有文件（对应 FileHelper.DeleteExcept）
/// （注意：操作有风险，需确保路径正确）
/// 目录不存在 → false；递归清理子目录，子目录清空后删除；文件扩展名（忽略大小写）与
/// keep_suffix 不同则删除。C# 的 Path.GetExtension 含前导点（如 ".jar"），此处保持一致。
/// 说明：C# 在 IO 失败时向上抛异常，Rust 版本记录 stderr 并返回 false
pub fn delete_except(folder_path: &str, keep_suffix: &str) -> bool {
    let dir = match fs::read_dir(folder_path) {
        Ok(d) => d,
        Err(_) => return false,
    };

    for entry in dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("删除失败：{e}");
                return false;
            }
        };
        let item_path = entry.path();

        if item_path.is_dir() {
            // 递归清理子目录
            if !delete_except(item_path.to_str().unwrap_or_default(), keep_suffix) {
                return false;
            }
            // 若子目录为空则删除
            let is_empty = item_path
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false);
            if is_empty {
                if let Err(e) = fs::remove_dir(&item_path) {
                    eprintln!("删除失败：{e}");
                    return false;
                }
            }
        } else {
            // 若文件后缀不是需要保留的，则删除
            let ext = item_path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            if !ext.eq_ignore_ascii_case(keep_suffix) {
                if let Err(e) = fs::remove_file(&item_path) {
                    eprintln!("删除失败：{e}");
                    return false;
                }
            }
        }
    }
    true
}
