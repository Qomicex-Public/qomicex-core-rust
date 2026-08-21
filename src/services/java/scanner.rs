//! Java 本地扫描（B7）
//!
//! 对应源文件：Services/JavaProvider.cs（Qomicex.Core.AOT）
//!
//! 拆分说明：源 `internal sealed class JavaProvider` 一个类三个职责——
//! 本文件（scanner.rs）只做**本地扫描**（`JavaScanner`）；
//! recommend.rs（`JavaRecommender`）与 download.rs（`JavaDownloader`）由
//! 并行批次翻译，本文件不引用他们。
//!
//! 覆盖方法（源方法名 → 本文件）：
//! - `Search` → `search`；`SearchQuick` → `search_quick`；`SearchDeep` → `search_deep`
//! - `SearchCustom` → `search_custom`；`ProcessResults` → `process_results`
//! - `BreadthFirstSearch` → `breadth_first_search`；`ShouldExclude` → `should_exclude`
//! - `SearchEnvironmentVariables` / `SearchRegistry` / `SearchHighPriorityPaths` /
//!   `SearchMinecraftRuntime` / `SearchPathEnvironment` → 同名 snake_case
//! - `AddJavaIfValid` / `GetJavaInfo` / `GetNormalizedMajorVersion` /
//!   `TryGetVersionFromCommand` / `GetJavaExecutablePath` / `GetValidDrives` → 同名 snake_case
//! - 常量：`ExcludedPaths` / `HighPriorityPaths` / `LinuxPaths` / `MacOSPaths` → 同名静态
//!
//! 关键移植决策：
//! - 并发：`Parallel.ForEach(MaxDegreeOfParallelism = 4)` → `for_each_parallel`
//!   （std::thread::scope + 原子工作索引，恰好 4 worker）；`ConcurrentBag` →
//!   `Mutex<Vec<JavaResult>>`；`ConcurrentDictionary<string,bool>` → `Mutex<HashSet<String>>`
//! - `Path.GetFullPath` → util/platform.rs 的 `normalize_path`（词法折叠、分隔符统一 '/'）
//! - `Trace.WriteLine` → `eprintln!`（项目既有约定，见 util/file_helper.rs）
//! - 注册表：零依赖 `reg query` 命令方案（主控决策，见 `search_registry` 文档）
//! - `search` 为同步实现（源 Task.FromResult 纯同步）；trait 的 async 包装由 facade 后置

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::models::java::{JavaResult, JavaSearchMode, JavaSearchOptions, JavaState, JavaType};
use crate::util::platform::normalize_path;

/// 排除目录集合（源：`ExcludedPaths` 静态字段，`StringComparer.OrdinalIgnoreCase`）。
/// C# 集合大小写不敏感 → 存储时统一小写，比较侧也小写（`should_exclude` / `search_custom`）。
static EXCLUDED_PATHS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    [
        "Windows",
        "ProgramData",
        "$Recycle.Bin",
        "System32",
        "SysWOW64",
        "WinSxS",
        "node_modules",
        ".git",
        ".svn",
        ".hg",
        "target",
        "build",
        "dist",
        ".gradle",
        ".m2",
        ".nuget",
        ".vscode",
        ".idea",
        "__pycache__",
        ".venv",
        "venv",
        "env",
        ".tox",
        ".pytest_cache",
        ".cargo",
        ".rustup",
        ".npm",
        ".yarn",
        ".pnpm-store",
        ".next",
        ".nuxt",
        "out",
        ".output",
        ".parcel-cache",
        ".webpack",
        ".cache",
        ".angular",
        ".svelte-kit",
        ".nyc_output",
        ".coverage",
        ".sonarqube",
        ".scannerwork",
        ".vs",
        ".vscode-test",
        "obj",
        "Steam",
        "Epic Games",
        "Origin",
        "EA Games",
        "Battle.net",
        "Ubisoft Game Launcher",
        "GOG Galaxy",
        "Temp",
        "tmp",
        "temp",
        "Downloads",
        "Prefetch",
        "Recent",
        "Cookies",
        "History",
        "INetCache",
        "Docker",
        "containerd",
    ]
    .into_iter()
    .map(str::to_lowercase)
    .collect()
});

/// Windows 高优先级路径（源：`HighPriorityPaths` 惰性属性，基于 SpecialFolder 组合）。
/// C# `Environment.GetFolderPath` 失败返回空串 → `Path.Combine("", "Java")` = 相对路径 "Java"；
/// 此处 env 缺失时 `unwrap_or_default()` 同语义保留。
static HIGH_PRIORITY_PATHS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let pf = env_dir("ProgramFiles");
    let pf86 = env_dir("ProgramFiles(x86)");
    let local_app_data = env_dir("LOCALAPPDATA");
    let app_data = env_dir("APPDATA");
    let user_profile = env_user_profile();
    let common_app_data = env_dir("ProgramData");
    let system_drive = system_drive_root();
    vec![
        join_path(&pf, "Java"),
        join_path(&pf, "Eclipse Adoptium"),
        join_path(&pf, "Eclipse Foundation"),
        join_path(&pf, "Amazon Corretto"),
        join_path(&join_path(&pf, "Microsoft"), "jdk"),
        join_path(&pf, "BellSoft"),
        join_path(&pf, "Semeru"),
        join_path(&pf, "Zulu"),
        join_path(&pf, "SapMachine"),
        join_path(&pf, "RedHat"),
        join_path(&pf, "ojdkbuild"),
        join_path(&pf, "GraalVM"),
        join_path(&pf, "Liberica"),
        join_path(&pf, "Temurin"),
        join_path(&pf86, "Java"),
        join_path(&pf86, "Eclipse Adoptium"),
        join_path(&local_app_data, "JetBrains"),
        join_path(&pf, "JetBrains"),
        join_path(&pf, "Android"),
        join_path(&system_drive, "Android"),
        join_path(&user_profile, ".jdks"),
        join_path(&join_path(&local_app_data, "Programs"), "Java"),
        join_path(&join_path(&user_profile, "scoop"), "apps"),
        join_path(&join_path(&system_drive, "tools"), "java"),
        join_path(&join_path(&common_app_data, "chocolatey"), "lib"),
        // 官方 Minecraft 启动器自带 runtime（%APPDATA%\.minecraft\runtime\{版本}\bin\java.exe）
        join_path(&app_data, ".minecraft/runtime"),
    ]
});

/// Linux 高优先级路径（源：`LinuxPaths` 静态字段，逐字保留）。
static LINUX_PATHS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let home = env_user_profile();
    vec![
        "/usr/lib/jvm".to_string(),
        "/usr/java".to_string(),
        "/opt/java".to_string(),
        "/usr/local/java".to_string(),
        "/snap".to_string(),
        "/var/snap".to_string(),
        join_path(&home, ".sdkman/candidates/java"),
        join_path(&home, ".jabba/jdk"),
        join_path(&home, ".asdf/installs/java"),
        join_path(&home, ".jenv/versions"),
        "/usr/lib64/jvm".to_string(),
        "/usr/local/lib/jvm".to_string(),
        "/opt/jdk".to_string(),
        "/opt/jre".to_string(),
        "/usr/local/jdk".to_string(),
        "/usr/local/jre".to_string(),
    ]
});

/// macOS 高优先级路径（源：`MacOSPaths` 静态字段，逐字保留）。
static MACOS_PATHS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let home = env_user_profile();
    vec![
        "/Library/Java/JavaVirtualMachines".to_string(),
        "/System/Library/Java/JavaVirtualMachines".to_string(),
        "/opt/homebrew/opt".to_string(),
        "/usr/local/opt".to_string(),
        join_path(&home, ".sdkman/candidates/java"),
        join_path(&home, ".jabba/jdk"),
        join_path(&home, ".asdf/installs/java"),
        join_path(&home, ".jenv/versions"),
        "/usr/local/Cellar/openjdk".to_string(),
        "/opt/local/Library/Java".to_string(),
        "/usr/libexec/java_home".to_string(),
    ]
});

/// 环境变量读取（C# `Environment.GetFolderPath` 的对应物：缺失 → 空串）。
fn env_dir(name: &str) -> String {
    std::env::var_os(name)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// 用户目录：Windows 取 USERPROFILE，Unix 取 HOME（对应 SpecialFolder.UserProfile）。
fn env_user_profile() -> String {
    if cfg!(windows) {
        env_dir("USERPROFILE")
    } else {
        env_dir("HOME")
    }
}

/// 系统盘根目录（对应 C# `Path.GetPathRoot(SpecialFolder.System) ?? @"C:\"`）：
/// SystemRoot（C:\Windows）取盘符前缀 → "C:\"。
fn system_drive_root() -> String {
    #[cfg(windows)]
    {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let s = system_root.to_string_lossy();
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            format!("{}\\", &s[..2])
        } else {
            "C:\\".to_string()
        }
    }
    #[cfg(not(windows))]
    {
        "C:\\".to_string()
    }
}

/// 路径拼接（对应 `Path.Combine`；base 为空时结果为相对路径，语义同 C#）。
fn join_path(base: &str, sub: &str) -> String {
    Path::new(base).join(sub).to_string_lossy().into_owned()
}

/// Java 扫描器（源：`JavaProvider` 的扫描职责拆分）。
///
/// `#[allow(dead_code)]`：facade 组合（api/java.rs 的 JavaProvider trait 实现）
/// 尚未接线，构造与 search 暂未被调用（源 JavaProvider 被核心调用；接线后移除）。
///
/// 字段说明：`http_client` 对应源 `_http`、`mirror` 对应源构造参数 `DownloadMirror`——
/// 扫描逻辑本身均不使用（纯本地探测），保留二者以与 recommend.rs / download.rs
/// 保持同一构造契约（共享注入）。C# 提供版构造器为 `JavaProvider(HttpClient http)`，
/// 任务指令要求扫描器构造另含 DownloadMirror 参数，按任务实现。
#[allow(dead_code)]
pub(crate) struct JavaScanner {
    /// HTTP 客户端（源：`JavaProvider._http`；扫描不使用，供统一注入契约）
    #[allow(dead_code)]
    http_client: reqwest::Client,
    /// 镜像偏好（源：构造参数 DownloadMirror；扫描不使用，供下载源选用契约）
    #[allow(dead_code)]
    mirror: DownloadMirror,
}

#[allow(dead_code)]
impl JavaScanner {
    /// 构造扫描器（源：`JavaProvider(HttpClient http, DownloadMirror mirror)`）。
    pub(crate) fn new(http_client: reqwest::Client, preferred_mirror: DownloadMirror) -> Self {
        Self {
            http_client,
            mirror: preferred_mirror,
        }
    }

    /// 按搜索选项扫描本机 Java 环境（源：`Search`）。
    ///
    /// - `options == null`（ArgumentNullException）→ `&JavaSearchOptions` 不可空，无映射；
    /// - Custom 模式缺 CustomRootPath（ArgumentException）→ `Err(Error::Params)`（消息逐字）；
    /// - 未知模式（ArgumentOutOfRangeException）→ Rust 闭枚举不可达。
    ///
    /// 同步实现（源为 Task.FromResult 纯同步逻辑）；trait（api/java.rs）的
    /// async 包装由 facade 后置负责。
    pub(crate) fn search(&self, options: &JavaSearchOptions) -> Result<Vec<JavaResult>, Error> {
        if options.mode == JavaSearchMode::Custom
            && options
                .custom_root_path
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(Error::Params {
                message: "Custom模式必须提供CustomRootPath".to_string(),
                source: None,
            });
        }

        Ok(match options.mode {
            JavaSearchMode::Quick => self.search_quick(options),
            JavaSearchMode::Deep => self.search_deep(options),
            JavaSearchMode::Custom => self.search_custom(options),
        })
    }

    /// 快速搜索（源：`SearchQuick`）。
    ///
    /// 各扫描源调用顺序逐字保留：
    /// 环境变量（JAVA_HOME/JDK_HOME/JRE_HOME）→ 注册表（仅 Windows）→
    /// 高优先级路径（按 OS 选路径表）→ Minecraft runtime（GameDir 非空时）→ PATH。
    fn search_quick(&self, options: &JavaSearchOptions) -> Vec<JavaResult> {
        let results: Mutex<Vec<JavaResult>> = Mutex::new(Vec::new());
        let discovered_paths: Mutex<HashSet<String>> = Mutex::new(HashSet::new());

        search_environment_variables(&results, &discovered_paths, options);

        if cfg!(windows) {
            search_registry(&results, &discovered_paths, options);
        }

        search_high_priority_paths(&results, &discovered_paths, options);

        if let Some(game_dir) = options.game_dir.as_deref() {
            if !game_dir.is_empty() {
                search_minecraft_runtime(game_dir, &results, &discovered_paths, options);
            }
        }

        search_path_environment(&results, &discovered_paths, options);

        process_results(&results.into_inner().unwrap(), options)
    }

    /// 深度搜索（源：`SearchDeep`）。
    ///
    /// 先跑快速搜索并把结果路径全部标记为已发现，再对有效驱动器根目录
    /// 并行（4 worker）做 BFS；每个驱动器的 BFS 内部已吞错，源外层
    /// `catch (Exception ex)` 的 Trace 在 Rust 无等价物（无异常传播），等义省略。
    fn search_deep(&self, options: &JavaSearchOptions) -> Vec<JavaResult> {
        let results: Mutex<Vec<JavaResult>> = Mutex::new(Vec::new());
        let discovered_paths: Mutex<HashSet<String>> = Mutex::new(HashSet::new());

        let quick_results = self.search_quick(options);
        for java in quick_results {
            discovered_paths
                .lock()
                .unwrap()
                .insert(normalize_path(&java.path));
            results.lock().unwrap().push(java);
        }

        let drives = get_valid_drives(options.include_network_drives);
        for_each_parallel(&drives, |drive| {
            breadth_first_search(drive, &results, &discovered_paths, options, &EXCLUDED_PATHS);
        });

        process_results(&results.into_inner().unwrap(), options)
    }

    /// 自定义路径搜索（源：`SearchCustom`）。
    ///
    /// 差异：源在 SearchCustom 开头重复校验 CustomRootPath（Search 已校验，
    /// 经 search() 进入时不可达）→ 等义省略；其余逐字保留：
    /// 根路径不存在 → 空列表；exclude = 内置 ExcludedPaths + CustomExcludePaths
    /// （OrdinalIgnoreCase → 小写）；单根 BFS（无并行）。
    fn search_custom(&self, options: &JavaSearchOptions) -> Vec<JavaResult> {
        let Some(root_path) = options.custom_root_path.as_deref() else {
            return Vec::new();
        };
        if !Path::new(root_path).is_dir() {
            return Vec::new();
        }

        let mut excludes: HashSet<String> = EXCLUDED_PATHS.clone();
        for path in &options.custom_exclude_paths {
            excludes.insert(path.to_lowercase());
        }

        let results: Mutex<Vec<JavaResult>> = Mutex::new(Vec::new());
        let discovered_paths: Mutex<HashSet<String>> = Mutex::new(HashSet::new());

        breadth_first_search(root_path, &results, &discovered_paths, options, &excludes);

        process_results(&results.into_inner().unwrap(), options)
    }
}

/// 结果整理（源：`ProcessResults`）：有效（State == Valid）在前，同状态按
/// MajorVersion 降序，取前 MaxResults 条。LINQ OrderBy/ThenBy 稳定排序 →
/// Rust `sort_by` 同为稳定排序，同序。`Take(count)` 对 count <= 0 返回空
/// （C# Enumerable.Take 语义）。
fn process_results(results: &[JavaResult], options: &JavaSearchOptions) -> Vec<JavaResult> {
    let mut sorted = results.to_vec();
    sorted.sort_by(|a, b| {
        (a.state != JavaState::Valid)
            .cmp(&(b.state != JavaState::Valid))
            .then(b.major_version.cmp(&a.major_version))
    });
    if options.max_results <= 0 {
        return Vec::new();
    }
    sorted.truncate(options.max_results as usize);
    sorted
}

/// 广度优先目录搜索（源：`BreadthFirstSearch`，特殊兼容，逐字保留）。
///
/// - 队列 (path, depth)，根 depth = 0；`while queue 非空 && results.len() < MaxResults`；
/// - `depth > MaxDepth` → 跳过该节点（根为 0，即最多下钻 MaxDepth 层）；
/// - 当前目录命中 java 可执行文件 → AddJavaIfValid(`BFS:{root}`) 后 **continue**
///   （不下钻子目录，与源一致）；
/// - 子目录过滤顺序：ShouldExclude → 隐藏目录（scan_hidden_folders=false 时跳过）→ 入队；
/// - 错误：目录读取失败 → eprintln（源 UnauthorizedAccessException 静默吞掉 +
///   其余异常 Trace.WriteLine；Rust 合并为日志后继续）。
fn breadth_first_search(
    root_path: &str,
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
    excludes: &HashSet<String>,
) {
    let mut queue: VecDeque<(String, i32)> = VecDeque::new();
    queue.push_back((root_path.to_string(), 0));

    while !queue.is_empty() {
        if results.lock().unwrap().len() >= options.max_results as usize {
            break;
        }
        let Some((current_path, depth)) = queue.pop_front() else {
            break;
        };

        if depth > options.max_depth {
            continue;
        }

        if let Some(java_path) = get_java_executable_path(&current_path) {
            add_java_if_valid(
                &java_path,
                results,
                discovered_paths,
                options,
                &format!("BFS:{root_path}"),
            );
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&current_path) else {
            eprintln!("BFS 扫描 {current_path} 失败: 读取目录失败");
            continue;
        };

        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let sub_dir = entry.path();
            let dir_name = sub_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            if should_exclude(sub_dir.to_string_lossy().as_ref(), &dir_name, excludes) {
                continue;
            }

            if !options.scan_hidden_folders && is_hidden(&sub_dir) {
                continue;
            }

            queue.push_back((sub_dir.to_string_lossy().into_owned(), depth + 1));
        }
    }
}

/// 目录是否应排除（源：`ShouldExclude`，逐字保留）。
///
/// 1. 目录名在 exclude 集合中（大小写不敏感 → 统一小写）；
/// 2. **任一 exclude 子串出现在完整路径任意位置即排除**（源
///    `fullPath.IndexOf(exclude, OrdinalIgnoreCase) >= 0`；"build" 等短名
///    误伤路径的行为 bug-for-bug 保留）；
/// 3. Windows 下以 `\\`（UNC）开头 → 排除；
/// 4. 点目录排除，白名单 `.jdks/.sdkman/.jenv/.jabba/.asdf`。
fn should_exclude(full_path: &str, dir_name: &str, excludes: &HashSet<String>) -> bool {
    let dir_name_lower = dir_name.to_lowercase();
    if excludes.contains(&dir_name_lower) {
        return true;
    }

    let full_path_lower = full_path.to_lowercase();
    for exclude in excludes {
        if full_path_lower.contains(exclude.as_str()) {
            return true;
        }
    }

    if cfg!(windows) && full_path.starts_with("\\\\") {
        return true;
    }

    if dir_name.starts_with('.')
        && !dir_name.eq_ignore_ascii_case(".jdks")
        && !dir_name.eq_ignore_ascii_case(".sdkman")
        && !dir_name.eq_ignore_ascii_case(".jenv")
        && !dir_name.eq_ignore_ascii_case(".jabba")
        && !dir_name.eq_ignore_ascii_case(".asdf")
    {
        return true;
    }

    false
}

/// 环境变量扫描（源：`SearchEnvironmentVariables`）：JAVA_HOME / JDK_HOME / JRE_HOME，
/// 目录存在 → 探测 java 可执行文件 → AddJavaIfValid（discoveredBy = 变量名）。
fn search_environment_variables(
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
) {
    for env_var in ["JAVA_HOME", "JDK_HOME", "JRE_HOME"] {
        let Some(path) = std::env::var_os(env_var) else {
            continue;
        };
        let path = path.to_string_lossy();
        if !path.is_empty() && Path::new(path.as_ref()).is_dir() {
            if let Some(java_path) = get_java_executable_path(path.as_ref()) {
                add_java_if_valid(&java_path, results, discovered_paths, options, env_var);
            }
        }
    }
}

/// Windows 注册表扫描（源：`SearchRegistry`，[SupportedOSPlatform("windows")] → cfg(windows)）。
///
/// ⚠️ 移植方案（主控决策）：Rust 无内置注册表 API，采用零依赖 `reg query` 命令
/// （std::process::Command）解析输出，不引入 winreg crate。
/// 逻辑结构逐字保留：16 个键路径 + 子键枚举 + JavaHome 值读取 + GetJavaExecutablePath 校验。
///
/// 注意：任务描述提及"HKLM/HKCU 的…路径"，但提供源码仅查询
/// `Registry.LocalMachine.OpenSubKey`（HKLM）→ 按源码实现 HKLM-only；
/// 如需 HKCU 需主控确认（16 键路径可复用于任何根键）。
///
/// ⚠️ 限制：reg 输出按系统 OEM 代码页编码（中文系统为 GBK），非 ASCII（CJK）路径
/// 经 from_utf8_lossy 会乱码 → 常规 Program Files 安装不受影响；
/// 修复需引入 winreg crate（主控决策）。
#[cfg(windows)]
fn search_registry(
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
) {
    const REGISTRY_KEYS: &[&str] = &[
        r"SOFTWARE\JavaSoft\Java Runtime Environment",
        r"SOFTWARE\JavaSoft\Java Development Kit",
        r"SOFTWARE\JavaSoft\JDK",
        r"SOFTWARE\WOW6432Node\JavaSoft\Java Runtime Environment",
        r"SOFTWARE\WOW6432Node\JavaSoft\Java Development Kit",
        r"SOFTWARE\WOW6432Node\JavaSoft\JDK",
        r"SOFTWARE\Eclipse Adoptium\JDK",
        r"SOFTWARE\Eclipse Adoptium\JRE",
        r"SOFTWARE\Microsoft\JDK",
        r"SOFTWARE\Amazon\Corretto",
        r"SOFTWARE\BellSoft\Liberica",
        r"SOFTWARE\Azul Systems\Zulu",
        r"SOFTWARE\AdoptOpenJDK\JDK",
        r"SOFTWARE\AdoptOpenJDK\JRE",
        r"SOFTWARE\Semeru\JDK",
        r"SOFTWARE\Semeru\JRE",
    ];

    for key_path in REGISTRY_KEYS {
        let Some(sub_keys) = reg_query_subkeys(key_path) else {
            continue; // 键不存在 → 源 OpenSubKey 返回 null → continue
        };
        for sub_key in sub_keys {
            let Some(java_home) = reg_query_value(key_path, &sub_key, "JavaHome") else {
                continue; // 值缺失 → 源 GetValue 返回 null → continue
            };
            if !java_home.is_empty() && Path::new(&java_home).is_dir() {
                if let Some(java_path) = get_java_executable_path(&java_home) {
                    add_java_if_valid(
                        &java_path,
                        results,
                        discovered_paths,
                        options,
                        &format!("Registry:{key_path}"),
                    );
                }
            }
        }
    }
}

/// 非 Windows 空实现（源：[SupportedOSPlatform("windows")] 属性语义）。
#[cfg(not(windows))]
fn search_registry(
    _results: &Mutex<Vec<JavaResult>>,
    _discovered_paths: &Mutex<HashSet<String>>,
    _options: &JavaSearchOptions,
) {
}

/// `reg query` 列出键的子键名（对应 `OpenSubKey` + `GetSubKeyNames`）。
///
/// 无 `/v` 时输出键路径首行 + 子键行（仅名字）+ 值条目行（含 REG_* 类型标记）：
/// ```text
/// HKEY_LOCAL_MACHINE\SOFTWARE\JavaSoft\JDK
///     jdk-17.0.1    REG_SZ    17
/// ```
/// 解析：首行之后，不含 REG_* 类型标记、非 "(default)" 的行视为子键名。
/// 退出码 0 = 成功；1 = ERROR_FILE_NOT_FOUND（键不存在，静默 = 源 OpenSubKey null）；
/// 其余退出码 → eprintln（对应源 Trace.WriteLine 异常路径）。
#[cfg(windows)]
fn reg_query_subkeys(key_path: &str) -> Option<Vec<String>> {
    let full_key = format!(r"HKLM\{key_path}");
    let output = std::process::Command::new("reg")
        .arg("query")
        .arg(&full_key)
        .output()
        .ok()?;
    if !output.status.success() {
        if output.status.code() != Some(1) {
            eprintln!(
                "读取注册表 {full_key} 失败: reg query 退出码 {:?}",
                output.status.code()
            );
        }
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut subkeys = Vec::new();
    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('(') {
            continue; // 空行 / "(default)" 等值条目
        }
        if trimmed.contains("REG_SZ")
            || trimmed.contains("REG_EXPAND_SZ")
            || trimmed.contains("REG_DWORD")
            || trimmed.contains("REG_QWORD")
            || trimmed.contains("REG_BINARY")
            || trimmed.contains("REG_MULTI_SZ")
            || trimmed.contains("REG_NONE")
        {
            continue; // 值条目（含类型标记）
        }
        subkeys.push(trimmed.to_string());
    }
    Some(subkeys)
}

/// `reg query ... /v JavaHome` 读取字符串值（对应 `subKey.GetValue("JavaHome")?.ToString()`）。
///
/// 输出行形如 `    JavaHome    REG_SZ    C:\Program Files\Java\jdk-17`——
/// 数据可能含空格 → 取类型标记之后的全部内容，再去掉两端引号。
#[cfg(windows)]
fn reg_query_value(key_path: &str, sub_key: &str, value_name: &str) -> Option<String> {
    let full_key = format!(r"HKLM\{key_path}\{sub_key}");
    let output = std::process::Command::new("reg")
        .arg("query")
        .arg(&full_key)
        .arg("/v")
        .arg(value_name)
        .output()
        .ok()?;
    if !output.status.success() {
        if output.status.code() != Some(1) {
            eprintln!(
                "读取注册表 {full_key} 失败: reg query 退出码 {:?}",
                output.status.code()
            );
        }
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut parts = line.trim().split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if !name.eq_ignore_ascii_case(value_name) {
            continue;
        }
        let Some(r#type) = parts.next() else {
            continue;
        };
        if !r#type.starts_with("REG_") {
            continue;
        }
        let data = parts.collect::<Vec<_>>().join(" ");
        if data.is_empty() {
            continue;
        }
        return Some(data.trim_matches('"').to_string());
    }
    None
}

/// 高优先级路径扫描（源：`SearchHighPriorityPaths`）：按 OS 选路径表，
/// 并行（4 worker）探测每个基路径下的一级子目录。
fn search_high_priority_paths(
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
) {
    let paths: &[String] = if cfg!(windows) {
        &HIGH_PRIORITY_PATHS
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        &LINUX_PATHS
    } else if cfg!(target_os = "macos") {
        &MACOS_PATHS
    } else {
        return;
    };

    for_each_parallel(paths, |base_path| {
        if !Path::new(base_path).is_dir() {
            return;
        }

        let Ok(entries) = std::fs::read_dir(base_path) else {
            eprintln!("扫描高优先级路径 {base_path} 失败: 读取目录失败");
            return;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let dir = entry.path().to_string_lossy().into_owned();
            if let Some(java_path) = get_java_executable_path(&dir) {
                add_java_if_valid(
                    &java_path,
                    results,
                    discovered_paths,
                    options,
                    &format!("HighPriority:{base_path}"),
                );
            }
        }
    });
}

/// Minecraft runtime 扫描（源：`SearchMinecraftRuntime`）：
/// `GameDir/runtime/{平台目录}/{版本目录}`，版本目录命中 java → AddJavaIfValid("MinecraftRuntime")。
/// 外层按平台目录并行（4 worker），内层逐版本目录串行（与源 Parallel.ForEach 结构一致）。
fn search_minecraft_runtime(
    game_dir: &str,
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
) {
    let runtime_path = Path::new(game_dir).join("runtime");
    if !runtime_path.is_dir() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(&runtime_path) else {
        eprintln!("扫描 Minecraft runtime 失败");
        return;
    };
    let platform_dirs: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();

    for_each_parallel(&platform_dirs, |platform_dir| {
        let Ok(version_entries) = std::fs::read_dir(platform_dir) else {
            eprintln!("扫描 Minecraft runtime {platform_dir} 失败: 读取目录失败");
            return;
        };
        for version_entry in version_entries {
            let Ok(version_entry) = version_entry else {
                continue;
            };
            if !version_entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let version_dir = version_entry.path().to_string_lossy().into_owned();
            if let Some(java_path) = get_java_executable_path(&version_dir) {
                add_java_if_valid(
                    &java_path,
                    results,
                    discovered_paths,
                    options,
                    "MinecraftRuntime",
                );
            }
        }
    });
}

/// PATH 环境扫描（源：`SearchPathEnvironment`）：PATH 按平台分隔符拆分
/// （Windows ';'，其余 ':'），并行（4 worker）探测每个条目。
///
/// bug-for-bug：源在 `EndsWith("bin")` 分支中 `Path.Combine(fullPath, javaName)`
/// 用的是 **fullPath 而非 parentDir**（与上一分支同路径，File.Exists 必然为 false，
/// 实为死代码）；parentDir 仅参与非空检查。逐字保留该行为。
fn search_path_environment(
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
) {
    let Some(path_var) = std::env::var_os("PATH") else {
        return;
    };
    let path_var = path_var.to_string_lossy();
    if path_var.is_empty() {
        return;
    }

    let separator = if cfg!(windows) { ';' } else { ':' };
    let paths: Vec<String> = path_var.split(separator).map(str::to_string).collect();

    for_each_parallel(&paths, |path_entry| {
        if path_entry.trim().is_empty() || !Path::new(path_entry).is_dir() {
            return;
        }

        let full_path = normalize_path(path_entry);
        let java_name = if cfg!(windows) { "java.exe" } else { "java" };
        let java_path = Path::new(&full_path).join(java_name);
        let java_path_str = java_path.to_string_lossy().into_owned();

        if java_path.is_file() {
            add_java_if_valid(&java_path_str, results, discovered_paths, options, "PATH");
        } else if full_path.to_lowercase().ends_with("bin") {
            let parent_dir = Path::new(&full_path).parent();
            if let Some(parent_dir) = parent_dir {
                let parent_dir = parent_dir.to_string_lossy();
                if !parent_dir.is_empty() {
                    let parent_java_path = Path::new(&full_path).join(java_name);
                    if parent_java_path.is_file() {
                        add_java_if_valid(
                            &parent_java_path.to_string_lossy(),
                            results,
                            discovered_paths,
                            options,
                            "PATH",
                        );
                    }
                }
            }
        }
    });
}

/// 校验并收集 Java（源：`AddJavaIfValid`）。
///
/// 1. GetFullPath（→ normalize_path）→ 已发现则跳过（去重，源为一次原子
///    ContainsKey+写入，此处同一锁内完成）；
/// 2. GetJavaInfo 探测（失败 → UnknownError 状态也照常收集）；
/// 3. IncludeJRE / IncludeJDK 过滤（Type == Unknown 不参与过滤，与源一致）。
fn add_java_if_valid(
    java_path: &str,
    results: &Mutex<Vec<JavaResult>>,
    discovered_paths: &Mutex<HashSet<String>>,
    options: &JavaSearchOptions,
    discovered_by: &str,
) {
    let normalized_path = normalize_path(java_path);

    {
        let mut discovered = discovered_paths.lock().unwrap();
        if discovered.contains(&normalized_path) {
            return;
        }
        discovered.insert(normalized_path.clone());
    }

    let Some(java_info) = get_java_info(&normalized_path, discovered_by) else {
        return;
    };

    if !options.include_jre && java_info.r#type == JavaType::JRE {
        return;
    }
    if !options.include_jdk && java_info.r#type == JavaType::JDK {
        return;
    }

    results.lock().unwrap().push(java_info);
}

/// 探测单个 Java 可执行文件（源：`GetJavaInfo`，逐字保留）。
///
/// - 文件不存在 → InvalidPath；javaHome（父父目录）缺失 → InvalidPath；
/// - 无 release 文件 → MissingReleaseFile + 命令回退；
/// - 解析 JAVA_VERSION / JAVA_RUNTIME_NAME / OS_ARCH / IMPLEMENTOR 四行
///   （`Split('=')[1]` → `.split('=').nth(1)`，值含 '=' 时丢后续段，bug-for-bug；
///   Trim('"') → `trim_matches('"')`）；IMPLEMENTOR ≠ "Oracle Corporation" 时
///   名称前缀实现商；Type 未知时按 jre/ include/ bin/javac(.exe) 判 JDK/JRE；
/// - 读取失败 → CorruptedReleaseFile + 命令回退。
fn get_java_info(java_path: &str, discovered_by: &str) -> Option<JavaResult> {
    let mut java_info = JavaResult {
        path: java_path.to_string(),
        major_version: 0,
        version: String::new(),
        state: JavaState::UnknownError,
        arch: String::new(),
        r#type: JavaType::Unknown,
        discovered_by: discovered_by.to_string(),
        name: "Java".to_string(),
    };

    if !Path::new(java_path).is_file() {
        java_info.state = JavaState::InvalidPath;
        return Some(java_info);
    }

    let Some(java_home) = Path::new(java_path).parent().and_then(Path::parent) else {
        java_info.state = JavaState::InvalidPath;
        return Some(java_info);
    };
    let java_home = java_home.to_string_lossy().into_owned();

    let release_file = Path::new(&java_home).join("release");
    if !release_file.is_file() {
        java_info.state = JavaState::MissingReleaseFile;
        if try_get_version_from_command(&mut java_info) {
            java_info.state = JavaState::Valid;
        }
        return Some(java_info);
    }

    match std::fs::read_to_string(&release_file) {
        Ok(content) => {
            for line in content.lines() {
                if line.starts_with("JAVA_VERSION=") {
                    let value = line.split('=').nth(1).unwrap_or_default();
                    let version = value.trim_matches('"');
                    java_info.version = version.to_string();
                    java_info.major_version = get_normalized_major_version(version);
                    java_info.name = format!("Java {version}");
                } else if line.starts_with("JAVA_RUNTIME_NAME=") {
                    let value = line.split('=').nth(1).unwrap_or_default();
                    let runtime_name = value.trim_matches('"');
                    if runtime_name.contains("JDK") {
                        java_info.r#type = JavaType::JDK;
                    } else if runtime_name.contains("JRE") {
                        java_info.r#type = JavaType::JRE;
                    }
                } else if line.starts_with("OS_ARCH=") {
                    let value = line.split('=').nth(1).unwrap_or_default();
                    java_info.arch = value.trim_matches('"').to_string();
                } else if line.starts_with("IMPLEMENTOR=") {
                    let value = line.split('=').nth(1).unwrap_or_default();
                    let implementor = value.trim_matches('"');
                    if !implementor.is_empty() && implementor != "Oracle Corporation" {
                        java_info.name = format!("{implementor} {}", java_info.name);
                    }
                }
            }

            if java_info.r#type == JavaType::Unknown {
                let javac = if cfg!(windows) { "javac.exe" } else { "javac" };
                if Path::new(&java_home).join("jre").is_dir()
                    || Path::new(&java_home).join("include").is_dir()
                    || Path::new(&java_home).join("bin").join(javac).is_file()
                {
                    java_info.r#type = JavaType::JDK;
                } else {
                    java_info.r#type = JavaType::JRE;
                }
            }
            java_info.state = JavaState::Valid;
        }
        Err(_) => {
            java_info.state = JavaState::CorruptedReleaseFile;
            if try_get_version_from_command(&mut java_info) {
                java_info.state = JavaState::Valid;
            }
        }
    }

    Some(java_info)
}

/// 版本号归一为大版本（源：`GetNormalizedMajorVersion`，逐字保留）：
/// 1.8 → 8（"1." 前缀 + 第二位数字）；空白 → -1；解析失败 → -1。
fn get_normalized_major_version(version: &str) -> i32 {
    if version.trim().is_empty() {
        return -1;
    }

    let parts: Vec<&str> = version.split('.').collect();
    if parts[0] == "1" && parts.len() > 1 {
        if let Ok(legacy_major) = parts[1].parse::<i32>() {
            return legacy_major;
        }
    }

    if let Ok(major) = parts[0].parse::<i32>() {
        return major;
    }

    -1
}

/// 命令回退获取版本（源：`TryGetVersionFromCommand`）：spawn `java -version`，
/// 读 stderr（java -version 输出到 stderr），解析引号内版本号；
/// 命中 → Version/MajorVersion/Name(`Java {version}`)。
///
/// 同时尝试检测 JDK/JRE 类型：通过 `java -version` 输出关键字或检查 `javac` 是否存在。
///
/// 超时差异：源 `ReadToEnd` 阻塞读 stderr → `WaitForExit(5000)` 超时**不杀进程**
/// （.NET 会泄漏悬挂 java 进程）；本实现 5 秒后 `child.kill()` 并回收读线程，
/// 防止进程悬挂时 join 永久阻塞。stderr 在子线程读取避免管道填满死锁。
///
/// 返回 true 表示成功获取版本（可设为 Valid），false 表示失败。
fn try_get_version_from_command(java_info: &mut JavaResult) -> bool {
    use std::io::Read;
    use std::path::Path;
    use std::process::{Command, Stdio};

    let java_path = &java_info.path;

    let Ok(mut child) = Command::new(java_path)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        eprintln!("通过命令获取 Java 版本失败: 无法启动进程");
        return false;
    };

    let stderr = child.stderr.take();
    let reader = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut buf = String::new();
                if let Some(mut stderr) = stderr {
                    let _ = stderr.read_to_string(&mut buf);
                }
                buf
            })
            .join()
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) | Err(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let output = reader.unwrap_or_default();

    if output.is_empty() {
        return false;
    }

    let mut detected_type = None;
    let mut parsed_version: Option<String> = None;

    for line in output.lines() {
        if line.contains("version") && parsed_version.is_none() {
            if let Some(version) = match_quoted_version(line) {
                parsed_version = Some(version);
            } else if let Some(version) = match_quoted_version_lenient(line) {
                parsed_version = Some(version);
            }
        }
        let lower = line.to_lowercase();
        if lower.contains("jdk") || lower.contains("java development kit") {
            detected_type = Some(crate::models::java::JavaType::JDK);
        } else if lower.contains("jre") || lower.contains("java runtime") {
            detected_type = Some(crate::models::java::JavaType::JRE);
        }
    }

    if let Some(version) = parsed_version {
        java_info.version = version.clone();
        java_info.major_version = get_normalized_major_version(&version);
        java_info.name = format!("Java {version}");
    } else {
        return false;
    }

    if detected_type.is_none() {
        if let Some(java_home) = Path::new(java_path).parent().and_then(Path::parent) {
            let javac = if cfg!(windows) { "javac.exe" } else { "javac" };
            if java_home.join("bin").join(javac).is_file()
                || java_home.join("include").is_dir()
                || java_home.join("jre").is_dir()
            {
                detected_type = Some(crate::models::java::JavaType::JDK);
            } else {
                detected_type = Some(crate::models::java::JavaType::JRE);
            }
        }
    }

    if let Some(t) = detected_type {
        java_info.r#type = t;
    }

    true
}

/// 宽松版本解析：允许下划线（如 1.8.0_502）、额外构建信息。
/// 仅在严格解析失败时作为二次尝试使用。
fn match_quoted_version_lenient(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            let mut end = pos + 1;
            let mut i = end;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
            if i == end {
                pos += 1;
                continue;
            }
            end = i;
            loop {
                let mut k = end;
                if k < bytes.len() && bytes[k] == b':' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'.' {
                    k += 1;
                } else {
                    break;
                }
                let digits_start = k;
                while k < bytes.len() && (bytes[k].is_ascii_digit() || bytes[k] == b'_') {
                    k += 1;
                }
                if k == digits_start {
                    break;
                }
                end = k;
            }
            if end < bytes.len() && bytes[end] == b'"' {
                return Some(line[pos + 1..end].to_string());
            }
        }
        pos += 1;
    }
    None
}

/// 源正则 `"(\d+(:?\.\d+)*)"` 的手工等价解析（bug-for-bug，特殊兼容）。
///
/// .NET 语义要点：
/// - `(:?…)` 不是非捕获组——`(:?` 实为组内**可选冒号** `:?` 后跟 `\.` → 每段为 `(:?\.\d+)`；
/// - 引号后必须紧跟数字（`\d+` 至少 1 位）；组后必须紧跟 `"`；
/// - `1.8.0_392` 因 `_392` 不满足 `(:?\.\d+)` → **与源一致不匹配**；
/// - 最左最长匹配：从每个 `"` 位置贪心取最长，其后是 `"` 才算命中
///   （较短回溯不可能以 `"` 结尾——引号不是 `(:?\.\d+)` 的起始字符）。
fn match_quoted_version(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            let mut end = pos + 1;
            let mut i = end;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == end {
                pos += 1;
                continue; // 引号后无数字 → `\d+` 失败，.NET 引擎同样不匹配
            }
            end = i;
            loop {
                let mut k = end;
                if k < bytes.len() && bytes[k] == b':' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'.' {
                    k += 1;
                } else {
                    break;
                }
                let digits_start = k;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k == digits_start {
                    break;
                }
                end = k;
            }
            if end < bytes.len() && bytes[end] == b'"' {
                return Some(line[pos + 1..end].to_string());
            }
        }
        pos += 1;
    }
    None
}

/// 探测 javaHome/bin 下的 java 可执行文件（源：`GetJavaExecutablePath`）：
/// Windows 为 java.exe，其余平台为 java；bin 目录不存在或文件不存在 → None。
fn get_java_executable_path(java_home: &str) -> Option<String> {
    let bin_dir = Path::new(java_home).join("bin");
    if !bin_dir.is_dir() {
        return None;
    }

    let java_name = if cfg!(windows) { "java.exe" } else { "java" };
    let java_path = bin_dir.join(java_name);
    if java_path.is_file() {
        Some(java_path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// 有效驱动器列表（源：`GetValidDrives`）。
///
/// - Windows：A-Z 盘符枚举 + GetDriveTypeW 类型过滤 + 根目录可访问性（近似 IsReady）；
/// - Linux/macOS：逐字返回 `/`、`/home`、`/opt`、`/usr`（不校验存在性，BFS 自行吞错）；
/// - 其余平台：空。
fn get_valid_drives(include_network_drives: bool) -> Vec<String> {
    let mut drives = get_windows_drives(include_network_drives);
    if cfg!(any(target_os = "linux", target_os = "android")) || cfg!(target_os = "macos") {
        drives.push("/".to_string());
        drives.push("/home".to_string());
        drives.push("/opt".to_string());
        drives.push("/usr".to_string());
    }
    drives
}

// Win32 GetDriveTypeW（kernel32，零依赖 FFI；edition 2024 unsafe extern 语法）。
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GetDriveTypeW"]
    fn get_drive_type_w(lp_root_path_name: *const u16) -> u32;
}

/// Windows 驱动器枚举（对应 `DriveInfo.GetDrives()` + IsReady + DriveType 过滤，逐字保留）。
///
/// 源过滤顺序：`!IsReady → continue`；`!includeNetworkDrives && Network → continue`；
/// `CDRom / Removable → continue`。本实现：
/// - DRIVE_NO_ROOT_DIR(1) → 不存在，跳过；DRIVE_REMOVABLE(2) / DRIVE_CDROM(5) → 跳过；
/// - DRIVE_REMOTE(4) → include_network_drives=false 时跳过；
/// - IsReady ≈ 根目录 `fs::metadata` 成功（断开的网络盘/无介质 → 失败 → 跳过，
///   与源行为一致；介质刚弹出的竞态窗口存在，可接受）。
#[cfg(windows)]
fn get_windows_drives(include_network_drives: bool) -> Vec<String> {
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;

    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", char::from(letter));
        let root_wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let drive_type = unsafe { get_drive_type_w(root_wide.as_ptr()) };

        match drive_type {
            0 | DRIVE_NO_ROOT_DIR => continue,
            DRIVE_REMOVABLE | DRIVE_CDROM => continue,
            DRIVE_REMOTE => {
                if !include_network_drives {
                    continue;
                }
            }
            _ => {}
        }

        if std::fs::metadata(&root).is_err() {
            continue;
        }
        drives.push(root);
    }
    drives
}

/// 非 Windows 空实现（源 Windows 分支在其余平台不执行）。
#[cfg(not(windows))]
fn get_windows_drives(_include_network_drives: bool) -> Vec<String> {
    Vec::new()
}

/// 目录是否隐藏（源：`File.GetAttributes` 的 `FileAttributes.Hidden` 位，Windows 专用）。
/// 元数据读取失败 → false（源抛异常后由外层 catch 吞掉 → 跳过该目录；
/// 差异：此处继续处理其余目录，仅影响目录删除竞态窗口）。
#[cfg(windows)]
fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|m| m.file_attributes() & 0x2 != 0)
        .unwrap_or(false)
}

/// 非 Windows 恒 false（源 FileAttributes.Hidden 在 Unix 恒为 Normal）。
#[cfg(not(windows))]
fn is_hidden(_path: &Path) -> bool {
    false
}

/// 并行遍历（对应 `Parallel.ForEach` + `MaxDegreeOfParallelism = 4`）。
///
/// std::thread::scope + 原子工作索引：恰好 4 个 worker 并发取任务
/// （条目不足 4 时按实际条数）；所有失败由回调内部吞掉/记录（源语义）。
fn for_each_parallel<T, F>(items: &[T], f: F)
where
    T: Sync,
    F: Fn(&T) + Sync,
{
    use std::sync::atomic::{AtomicUsize, Ordering};

    let workers = items.len().min(4);
    if workers == 0 {
        return;
    }

    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= items.len() {
                        break;
                    }
                    f(&items[index]);
                }
            });
        }
    });
}
