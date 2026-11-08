// Copyright (C) 2026 Qomicex Launcher Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! 整合包安装器（B13）
//!
//! 对应源目录：Qomicex.Core.AOT/Services/Installers/Modpacks/
//! - CurseForgeModpackInstaller.cs（109 行）→ curseforge.rs
//! - ModrinthModpackInstaller.cs（100 行）→ modrinth.rs
//! - FTBModpackInstaller.cs（77 行）→ ftb.rs
//! - ModpackModels.cs（40 行）→ models.rs
//!
//! 三个安装器均实现 `Installer` trait（installer.rs，B9 契约）；
//! 由 InstallerFactory（factory.rs）的 3 个 create_modpack 方法创建
//! （工厂接线在 B13 后续步骤完成，当前仍为占位实现）。
//! 本目录整体在 services/ 实现层，全部 `pub(crate)`。

pub mod curseforge;
pub mod ftb;
pub mod modrinth;

/// 把 zip 条目名解析成 `base` 之下的安全路径；越界（路径穿越 / 绝对路径）→ None。
///
/// 整合包的 overrides 条目名来自**不可信的 zip**：`overrides/../../x` 这类名字在去掉
/// 前缀后仍是相对路径，直接 `base.join()` 会写到安装目录之外。这里逐段过滤，
/// 拒绝 `..`、盘符段与空段，并在拼完后复核结果仍在 `base` 之内。
///
/// `C:payload` 这类**盘符相对**段也必须拒绝：`PathBuf::push` 遇到它会把整条路径替换成
/// 「当前 C 盘目录下的 payload」，`starts_with(base)` 拦不住（评审 sourcery 指出）。
///
/// 段尾的点与空格同样要按 Windows 口径归一后再判：`..junk`、`...`、`..  ` 在 Win32 上会
/// 被规范化成父目录段，只看字面量 `== ".."` 会漏（评审 sourcery 指出）。
pub(crate) fn safe_join_under(
    base: &std::path::Path,
    relative_path: &str,
) -> Option<std::path::PathBuf> {
    let mut out = base.to_path_buf();
    // 两种分隔符都可能是分隔符（zip 规范要求 `/`，实际样本里 `\` 也出现）
    for segment in relative_path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        // Windows 的段规范化会剥掉尾部的点与空格，所以 `..junk`、`...`、`..  ` 都要先归一。
        // 归一后的形态只有三种可能：空（纯点/空格，等价 `.`）、以点开头（父目录段）、正常名。
        let normalized = segment.trim_end_matches(['.', ' ']);
        if normalized.is_empty() {
            // 纯点/空格段：`...` / `   ` 归一后为空，等价于 `.` → 丢弃；
            // 但 `..` 本身也会被剥成空串，不能当无害段放过，故单独确认没有两个连续点
            if segment.contains("..") {
                return None;
            }
            continue;
        }
        // `..`、`..junk` 归一后仍以 `..` 开头 → 父目录段
        if normalized.starts_with("..") {
            return None;
        }
        // 盘符前缀（`C:`、`C:payload`）与 UNC 共享名都是绝对/盘符相对的起点
        if has_drive_prefix(segment) {
            return None;
        }
        out.push(segment);
    }
    if out.starts_with(base) {
        Some(out)
    } else {
        None
    }
}

/// 该段是否带 Windows 盘符前缀（`C:` 或 `C:rest`）。
fn has_drive_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// zip 条目名是否为目录条目（按 `/` 或 `\` 结尾）。
///
/// 必须两种分隔符都认：`overrides/subdir\` 若只按 `/` 取末段会得到非空文件名，
/// 随后被当成文件用 `File::create` 创建，等它的子条目写入时才失败（评审 sourcery 指出）。
pub(crate) fn is_directory_entry(entry_name: &str) -> bool {
    entry_name.ends_with(['/', '\\'])
}

/// 解压预算：以**目标盘剩余空间**为上限，留出安全余量。
///
/// 整合包的 override 条目来自不可信 zip，高压缩比条目（几十 KB 压出数 GB）可以把目标盘
/// 写满并留下半成品安装（issue #12）。上限取「安装开始时目标盘可用空间」减去余量：
/// 只按压缩包体积或固定常量封顶，都会在「盘本来就快满」或「大型良性整合包」两头上出错。
///
/// 余量用来给安装过程本身留出空间（下载库、解压后的临时文件、并发进行的其它写操作）。
pub(crate) struct ExtractBudget {
    /// 还能写多少字节（安装开始时的剩余空间 − 余量）
    remaining: u64,
    /// 本次安装已写字节数（诊断用）
    written: u64,
}

/// 预留余量：安装过程中还会有库下载等写入，不能把剩余空间算到 0。
const EXTRACT_RESERVED_HEADROOM: u64 = 256 * 1024 * 1024;

/// 单个条目允许写出的上限（声明解压大小），挡住压缩包里单条异常巨文件。
/// 按剩余空间封顶时这条仍必要：剩余空间可能很大，而单个条目不该独占全部。
const MAX_SINGLE_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

impl ExtractBudget {
    /// 按目标目录所在盘的剩余空间建立预算；探测失败 → None（调用方按「无法确定」处理）。
    pub(crate) fn for_target(target_dir: &std::path::Path) -> Option<Self> {
        let free = free_space_bytes(target_dir)?;
        Some(Self {
            remaining: free.saturating_sub(EXTRACT_RESERVED_HEADROOM),
            written: 0,
        })
    }

    /// 声明该条目将写出 `declared` 字节；超限返回 Err（调用方据此让整包安装失败）。
    pub(crate) fn reserve(&mut self, declared: u64) -> Result<(), String> {
        if declared > MAX_SINGLE_ENTRY_BYTES {
            return Err(format!(
                "单个条目声明解压 {declared} 字节，超过上限 {MAX_SINGLE_ENTRY_BYTES} 字节"
            ));
        }
        if declared > self.remaining {
            return Err(format!(
                "解压所需空间不足：本条需 {declared} 字节，剩余预算 {} 字节",
                self.remaining
            ));
        }
        self.remaining -= declared;
        self.written += declared;
        Ok(())
    }

    /// 已写字节数（诊断/测试用）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn written(&self) -> u64 {
        self.written
    }
}

/// 目标目录所在盘的可用字节数。
///
/// 标准库不提供 `statvfs`/`GetDiskFreeSpaceEx`，故用「向目标目录写一个探针文件并读回」
/// 无法得到真实余量，只能借助平台 API：Windows 走 `GetDiskFreeSpaceExW`，
/// Unix 走 `statvfs`。两者都用系统调用，失败 → None。
#[cfg(windows)]
fn free_space_bytes(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            lp_directory_name: *const u16,
            lp_free_bytes_available_to_caller: *mut u64,
            lp_total_number_of_bytes: *mut u64,
            lp_total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }

    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut available: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    // SAFETY: `wide` 以 NUL 结尾且在本调用期间存活；三个输出指针均指向本栈帧内的 u64。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 { None } else { Some(available) }
}

#[cfg(unix)]
fn free_space_bytes(dir: &std::path::Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    #[repr(C)]
    struct StatVfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        // 其余字段布局随平台而异，本函数不读它们，留足空间避免越界写
        _reserved: [u64; 11],
    }

    unsafe extern "C" {
        fn statvfs(path: *const std::ffi::c_char, buf: *mut StatVfs) -> i32;
    }

    let c_path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<StatVfs>::zeroed();
    // SAFETY: `c_path` 以 NUL 结尾；buf 提供足够大的可写区域（Linux 上 struct statvfs 更小）。
    let rc = unsafe { statvfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: rc == 0 表示调用成功并已填充 buf
    let vfs = unsafe { buf.assume_init() };
    let block = if vfs.f_frsize != 0 {
        vfs.f_frsize
    } else {
        vfs.f_bsize
    };
    Some(vfs.f_bavail.saturating_mul(block))
}

#[cfg(not(any(windows, unix)))]
fn free_space_bytes(_dir: &std::path::Path) -> Option<u64> {
    None
}

/// 解开符号链接后确认 `path` 仍位于 `root` 之下。
///
/// `safe_join_under` 只做词法校验，挡不住「安装目录里已被预先放了指向外部的链接」
/// ——`create_dir_all` / 文件写入会跟随它写出根目录（issue #12）。
/// 这里对**已存在的最深祖先**求真实路径再比对；不存在的尾部（本次要新建的部分）
/// 不可能已是链接，无需解析。
pub(crate) fn resolves_within_root(root: &std::path::Path, path: &std::path::Path) -> bool {
    let Ok(real_root) = root.canonicalize() else {
        // 根目录本身解析不了（尚未创建）→ 无从判定，交给调用方按失败处理
        return false;
    };
    // 找到已存在的最近祖先（目标可能还没创建）
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return false,
        }
    }
    match probe.canonicalize() {
        Ok(real) => real.starts_with(&real_root),
        Err(_) => false,
    }
}

/// 一次整合包解压的上下文：统一承载路径校验、越界策略、预算与半成品清理。
///
/// 两个安装器（CurseForge / Modrinth）的 overrides 释放逻辑完全同构，差异只在
/// 「哪些前缀算覆盖目录」，故把循环体收在这里，避免两份实现各自漂移
/// （issue #12 的四项加固只修一处）。
pub(crate) struct OverrideExtractor<'a> {
    root: &'a std::path::Path,
    budget: ExtractBudget,
    /// 本次新建的文件与目录：失败时删除（这些删掉不损失用户数据）
    created: Vec<std::path::PathBuf>,
    /// 本次**覆盖**的既有文件：失败时保留，不能删——删了才是真丢用户数据
    overwritten: Vec<std::path::PathBuf>,
}

impl<'a> OverrideExtractor<'a> {
    /// 建解压上下文；目标盘剩余空间探测失败 → Err（宁可安装失败，也不在未知余量下写盘）。
    pub(crate) fn new(root: &'a std::path::Path) -> Result<Self, String> {
        let budget = ExtractBudget::for_target(root).ok_or_else(|| {
            format!(
                "无法确定目标磁盘剩余空间，拒绝解压整合包：{}",
                root.display()
            )
        })?;
        Ok(Self {
            root,
            budget,
            created: Vec::new(),
            overwritten: Vec::new(),
        })
    }

    /// 释放一条 overrides 条目。
    ///
    /// 越界条目**整包失败**（issue #12 选定策略）：静默跳过会让调用方误以为安装完整，
    /// 而良性整合包几乎不会出现越界条目——真出现就说明包本身不可信。
    pub(crate) fn extract_entry<R: std::io::Read + std::io::Seek>(
        &mut self,
        entry: &mut zip::read::ZipFile<'_, R>,
        full_name: &str,
        prefix_len: usize,
    ) -> Result<(), String> {
        let relative_path = &full_name[prefix_len..];
        let destination_path = safe_join_under(self.root, relative_path)
            .ok_or_else(|| format!("整合包条目越界，拒绝解压：{full_name}"))?;
        // 词法通过还不够：安装目录里可能已被放置指向外部的符号链接
        if !resolves_within_root(self.root, &destination_path) {
            return Err(format!(
                "整合包条目解引用后越出安装目录，拒绝解压：{full_name}"
            ));
        }

        if is_directory_entry(full_name) {
            if !destination_path.exists() {
                std::fs::create_dir_all(&destination_path)
                    .map_err(|e| format!("创建目录失败 {}: {e}", destination_path.display()))?;
                self.created.push(destination_path);
            }
            return Ok(());
        }

        // 预算按 zip 头声明的解压大小扣减；实际写入量在下面再卡一道
        let declared = entry.size();
        self.budget.reserve(declared)?;

        if let Some(parent) = destination_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
            self.created.push(parent.to_path_buf());
        }
        // 覆盖语义与源一致（ExtractToFile(overwrite: true)），但**不能先删原文件**：
        // 中途失败会连带丢掉用户原有的配置（ADR-124「失败不丢弃用户文件」）。
        // 改为写同目录临时文件再 rename 覆盖——要么仍是旧内容，要么是新内容。
        let mut tmp_name = destination_path
            .file_name()
            .unwrap_or_default()
            .to_os_string();
        tmp_name.push(format!(".{}.qml-tmp", std::process::id()));
        let tmp_path = destination_path.with_file_name(&tmp_name);
        // create_new：不跟随符号链接（临时名可预测，缓存目录可能被低权限用户写入）
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| format!("创建文件失败 {}: {e}", tmp_path.display()))?;
        // 实写量也受预算约束：zip 头可能撒谎，故按声明量 + 1 字节封顶
        let mut limited = std::io::Read::take(&mut *entry, declared.saturating_add(1));
        let written = std::io::copy(&mut limited, &mut out);
        drop(out);
        let written = match written {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(format!("写入失败 {}: {e}", destination_path.display()));
            }
        };
        if written > declared {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!(
                "条目实际解压 {written} 字节，超过声明 {declared} 字节"
            ));
        }
        // 记录目标是否本来存在：只有本次新建的才在回滚时删除
        let was_new = !destination_path.exists();
        if let Err(e) = std::fs::rename(&tmp_path, &destination_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("落盘失败 {}: {e}", destination_path.display()));
        }
        if was_new {
            self.created.push(destination_path);
        } else {
            // 覆盖了用户原有文件：内容已被换掉，无法还原旧内容。
            // 记下来便于失败时至少不误删该文件（删除才是真正的数据丢失）。
            self.overwritten.push(destination_path);
        }
        Ok(())
    }

    /// 本次已写字节数。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn written(&self) -> u64 {
        self.budget.written()
    }

    /// 删除本次安装**新建**的文件与目录（失败时调用，避免留下半成品）。
    ///
    /// 只删 `created`：`overwritten` 里是被本次覆盖的用户既有文件，删掉等于丢用户数据
    /// （ADR-124 已确立「安装失败不丢用户文件」）。它们的旧内容无法还原，但保留现状
    /// 仍好于删除——用户至少还有一份可用文件。
    /// 逆序删除：先文件后其父目录。
    pub(crate) fn rollback(&mut self) {
        for path in self.created.drain(..).rev() {
            if path.is_dir() {
                let _ = std::fs::remove_dir(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        self.overwritten.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::safe_join_under;
    use std::path::Path;

    #[test]
    fn safe_join_under_rejects_traversal() {
        let base = Path::new("install");
        // 正常条目照常解析
        assert_eq!(
            safe_join_under(base, "mods/a.jar"),
            Some(base.join("mods").join("a.jar"))
        );
        // 路径穿越一律拒绝（评审 sourcery 指出：source 直接 Combine 会写穿安装目录）
        for evil in [
            "../escape.jar",
            "mods/../../escape.jar",
            "mods/../../../etc/passwd",
            "..\\escape.jar",
            "mods\\..\\..\\escape.jar",
        ] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 属越界路径，应被拒绝"
            );
        }
        // Windows 盘符段不允许（否则 `C:x` 会脱离 base）
        assert!(safe_join_under(base, "C:/x.jar").is_none());
        // 盘符相对段（`C:payload`）也必须拒绝：PathBuf::push 会整条替换，
        // 末尾的 starts_with(base) 拦不住（评审 sourcery 指出）
        for evil in ["C:payload", "mods/D:escape.jar", "C:payload/x.jar"] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 属盘符相对路径，应被拒绝"
            );
        }
        // 空段与 `.` 折叠掉，不越界
        assert_eq!(
            safe_join_under(base, "./mods//a.jar"),
            Some(base.join("mods/a.jar"))
        );
    }

    #[test]
    fn safe_join_under_rejects_windows_normalized_parent_segments() {
        // Win32 的段规范化会剥掉尾部的点与空格，`..junk`、`.. `、`....` 都会被当成父目录段；
        // 只比对字面量 `== ".."` 会漏（评审 sourcery 指出）。
        let base = Path::new("install");
        for evil in [
            "..junk/x.jar",
            ".. /x.jar",
            "mods/.../../x.jar",
            "mods/.. /x.jar",
        ] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 归一后是父目录段，应被拒绝"
            );
        }
        // `...` 含连续两点，按 Win32 口径同样是父目录段 → 拒绝而不是放过
        assert!(
            safe_join_under(base, "mods/.../a.jar").is_none(),
            "`...` 含连续两点，应按父目录段拒绝"
        );
        // 纯空格的段归一后为空、且不含连续点 → 等价于 `.`，折叠掉
        assert!(
            safe_join_under(base, "mods/   /a.jar").is_some(),
            "纯空格段应折叠掉而不是拒绝"
        );
    }

    #[test]
    fn is_directory_entry_accepts_both_separators() {
        use super::is_directory_entry;
        // 两种分隔符结尾都算目录条目；否则 `subdir\` 会被当文件创建，
        // 等子条目写入时才失败（评审 sourcery 指出）
        assert!(is_directory_entry("overrides/subdir/"));
        assert!(is_directory_entry("overrides/subdir\\"));
        assert!(!is_directory_entry("overrides/subdir/file.jar"));
        assert!(!is_directory_entry("overrides/file.jar"));
    }

    #[test]
    fn has_drive_prefix_covers_drive_relative_segments() {
        use super::has_drive_prefix;
        // 盘符相对段（`C:payload`）必须被识别：这类段交给 PathBuf::push 会整条替换。
        // 在 Windows 上末尾的 starts_with 恰好也能拦，但依赖路径当前形态并不可靠
        // （base 为绝对路径时行为不同），故显式判定。
        for evil in ["C:", "C:payload", "D:escape.jar", "z:dir"] {
            assert!(has_drive_prefix(evil), "{evil:?} 应判为盘符前缀");
        }
        for ok in ["mods", "a.jar", "C", "dir:", ""] {
            assert!(!has_drive_prefix(ok), "{ok:?} 不应判为盘符前缀");
        }
    }

    // ── issue #12：解压预算 ────────────────────────────────────

    /// 造一个指定剩余预算的 ExtractBudget（跳过真实磁盘探测，便于精确测边界）。
    fn budget_with_remaining(remaining: u64) -> super::ExtractBudget {
        super::ExtractBudget {
            remaining,
            written: 0,
        }
    }

    #[test]
    fn budget_accepts_entries_within_remaining_space() {
        // 正常路径：小条目照常通过，预算按声明量扣减
        let mut b = budget_with_remaining(1024);
        assert!(b.reserve(512).is_ok());
        assert_eq!(b.written(), 512);
        assert!(b.reserve(512).is_ok(), "累计恰好用满是允许的");
        assert_eq!(b.written(), 1024);
    }

    #[test]
    fn budget_rejects_when_remaining_space_insufficient() {
        // 异常路径：累计超出剩余空间 → 拒绝（issue #12 的核心诉求：
        // 高压缩比条目不能把目标盘写满）
        let mut b = budget_with_remaining(1000);
        assert!(b.reserve(600).is_ok());
        let err = b.reserve(600).expect_err("剩余 400 字节，600 应被拒绝");
        assert!(err.contains("空间不足"), "错误信息应说明空间不足: {err}");
        // 被拒后预算不应被扣减
        assert_eq!(b.written(), 600, "失败的预约不得计入已写量");
    }

    #[test]
    fn budget_rejects_single_oversized_entry_even_with_plenty_of_room() {
        // 边界：剩余空间很大，但单条目超过 MAX_SINGLE_ENTRY_BYTES 仍应拒绝——
        // 否则一个条目就能独占整块盘
        let mut b = budget_with_remaining(super::MAX_SINGLE_ENTRY_BYTES * 10);
        let err = b
            .reserve(super::MAX_SINGLE_ENTRY_BYTES + 1)
            .expect_err("单条目超上限应被拒绝");
        assert!(err.contains("超过上限"), "错误信息应说明单条目超限: {err}");
        // 恰好等于上限则允许
        assert!(b.reserve(super::MAX_SINGLE_ENTRY_BYTES).is_ok());
    }

    #[test]
    fn extract_budget_reads_real_free_space() {
        // 真实探测：临时目录所在盘应能报出非零可用空间。
        // 探测失败会让安装直接报错，故必须真实可用。
        let dir = std::env::temp_dir();
        let budget = super::ExtractBudget::for_target(&dir).expect("临时目录应能探测出剩余空间");
        assert_eq!(budget.written(), 0);
        // 预留余量后仍应有正预算（否则本机盘已满，用例环境异常）
        assert!(budget.remaining > 0, "临时目录剩余预算应为正");
    }

    // ── issue #12：符号链接防护 ────────────────────────────────

    #[test]
    fn resolves_within_root_accepts_plain_paths() {
        let root = std::env::temp_dir().join(format!("qomicex-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("mods")).unwrap();
        let inside = root.join("mods").join("a.jar");
        assert!(
            super::resolves_within_root(&root, &inside),
            "根目录内的普通路径应通过"
        );
        // 尚未创建的尾部路径也应通过（其祖先在根内，尾部不可能已是链接）
        let future = root.join("mods").join("nested").join("b.jar");
        assert!(
            super::resolves_within_root(&root, &future),
            "未创建但祖先在根内的路径应通过"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn resolves_within_root_rejects_escaping_symlink() {
        // 异常路径：安装目录内预置指向外部的符号链接 → 必须判为越界
        let base = std::env::temp_dir().join(format!("qomicex-sym-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let escape = root.join("link").join("evil.jar");
        assert!(
            !super::resolves_within_root(&root, &escape),
            "经符号链接指向根目录之外时必须拒绝"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── issue #12：整包解压行为（端到端） ──────────────────────

    /// 造一个真实整合包 zip：按给定条目写文件。
    fn write_modpack(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    fn temp_install_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qomicex-modpack-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// CurseForge 整合包必需的 manifest.json（含 overrides 目录名）。
    const CURSEFORGE_MANIFEST: &[u8] =
        br#"{"manifestType":"minecraftModpack","overrides":"overrides"}"#;

    #[test]
    fn modpack_install_writes_override_files() {
        // 正常路径：合规整合包应把 overrides/ 下的文件原样释放
        let game_dir = temp_install_dir("ok");
        let pack = game_dir.join("pack.zip");
        write_modpack(
            &pack,
            &[
                ("manifest.json", CURSEFORGE_MANIFEST),
                ("overrides/options.txt", b"lang:zh_CN"),
                ("overrides/config/a.toml", b"k=1"),
            ],
        );
        let installer = super::curseforge::CurseForgeModpackInstaller::new(
            game_dir.to_str().unwrap(),
            false,
            pack.to_str().unwrap(),
        );
        installer.release_files_for_test("1.20.1").unwrap();

        assert_eq!(
            std::fs::read_to_string(game_dir.join("options.txt")).unwrap(),
            "lang:zh_CN"
        );
        assert_eq!(
            std::fs::read_to_string(game_dir.join("config").join("a.toml")).unwrap(),
            "k=1"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn rollback_keeps_preexisting_files_and_removes_only_new_ones() {
        // 回归：回滚**不得**删除用户原有的文件。
        // 原实现先无条件 remove_file 再 create，失败回滚时会把用户配置一起删掉；
        // 且旧内容当场就丢了。现改为临时文件 + rename，并在回滚时只删本次新建的。
        let game_dir = temp_install_dir("rollback-keep");
        // 用户已有一份配置
        std::fs::write(game_dir.join("existing.txt"), b"user-config").unwrap();
        let pack = game_dir.join("pack.zip");
        write_modpack(
            &pack,
            &[
                ("manifest.json", CURSEFORGE_MANIFEST),
                // 覆盖用户已有文件
                ("overrides/existing.txt", b"from-pack"),
                // 新建一个文件（失败时应被删）
                ("overrides/new.txt", b"new"),
                // 越界条目触发失败
                ("overrides/../escaped.txt", b"evil"),
            ],
        );
        let installer = super::curseforge::CurseForgeModpackInstaller::new(
            game_dir.to_str().unwrap(),
            false,
            pack.to_str().unwrap(),
        );
        installer
            .release_files_for_test("1.20.1")
            .expect_err("含越界条目应失败");

        assert!(
            game_dir.join("existing.txt").exists(),
            "回滚不得删除用户原有文件"
        );
        assert!(
            !game_dir.join("new.txt").exists(),
            "本次新建的文件应被回滚删除"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn modpack_install_fails_on_escaping_entry_and_rolls_back() {
        // issue #12 选定策略：越界条目让**整包失败**，并清掉本次已写入的半成品。
        // 静默跳过会让调用方误以为安装完整。
        let game_dir = temp_install_dir("escape");
        let pack = game_dir.join("pack.zip");
        write_modpack(
            &pack,
            &[
                ("manifest.json", CURSEFORGE_MANIFEST),
                // 先有一条合法条目（会被写入，失败后应被回滚）
                ("overrides/good.txt", b"ok"),
                // 再有一条越界条目
                ("overrides/../escaped.txt", b"evil"),
            ],
        );
        let installer = super::curseforge::CurseForgeModpackInstaller::new(
            game_dir.to_str().unwrap(),
            false,
            pack.to_str().unwrap(),
        );
        let err = installer
            .release_files_for_test("1.20.1")
            .expect_err("含越界条目的整合包必须安装失败");
        let msg = format!("{err}");
        assert!(msg.contains("越界"), "错误信息应说明越界: {msg}");

        // 回滚：合法条目写出的文件不应残留
        assert!(
            !game_dir.join("good.txt").exists(),
            "失败后已写入的半成品应被删除"
        );
        // 越界条目不得写到安装目录之外
        assert!(
            !game_dir.parent().unwrap().join("escaped.txt").exists(),
            "越界条目不得写到安装目录之外"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }
}
