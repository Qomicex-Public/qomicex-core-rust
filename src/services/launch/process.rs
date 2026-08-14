//! 进程启动 + natives 处理（B8，对应 LaunchExecutor.cs 的启动/进程/natives 部分）
//!
//! 拆分说明：源文件 Qomicex.Core.AOT/Services/LaunchExecutor.cs（835 行）按职责拆为两个文件：
//! - jvm_args.rs（另一 Translator）：`struct LaunchExecutor` 定义 + 参数组装方法
//!   （SelectParams / GetJVMParams / GetGameParams / GetClassPath / GetMainClass / NormalizeArg 等）
//! - 本文件（process.rs）：进程启动与 natives 处理（LaunchAsync / KillAsync / GetNativePath /
//!   UnzipNatives / IsForeignArchDir / FlattenNatives / ParseJavaLibraryPath / GetNatives /
//!   ParseGameJson，另含 catch 块使用的 GetDataDir）
//!
//! 协同契约（假定 jvm_args.rs 提供，签名详见翻译日志 p34）：
//! - `select_params(&self, options: &LaunchOptions) -> Result<String, Error>`
//! - `get_jvm_params(&self, options: &LaunchOptions) -> Result<Vec<String>, Error>`
//! - `normalize_arg(&self, value: &str) -> String`
//! - GameRoot 覆盖统一约定：源在 LaunchAsync 开头持久修改 `_gameDir` 字段（originalGameDir
//!   未恢复）；Rust `&self` 不可变，本文件与 jvm_args.rs 统一按"`options.game_root` 非空则
//!   覆盖 `self.game_dir`"处理（见 `effective_game_dir`），jvm_args.rs 读取版本 JSON / classpath
//!   时同样需遵守，否则带 GameRoot 启动时参数路径错误。
//!
//! 关键偏差（详见翻译日志 p34）：
//! - 源从不抛异常——失败以 `LaunchResult{Success=false, ProcessId=-1}` 表达；Rust 按
//!   api/launch.rs 契约以 `Err(Error)` 表达，消息保留"启动失败,{ex.Message}"模式，
//!   并保留 catch 块对 `logs/launch-errors.log` 的写入。
//! - 源 `Task.Run` 线程池；Rust 直接在 async 上下文执行阻塞代码（调用方应使用多线程 runtime）。
//! - 源输出回调（OnOutput/OnError/OnExit）在 B1 模型已省略；源本身只写控制台，
//!   此处同样直接写控制台，不转 event.rs 事件。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::future::OptionFuture;
use tokio::io::AsyncBufReadExt;

use crate::api::version::VersionLocator as _;
use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::models::launch::{LaunchOptions, LaunchResult};
use crate::models::version_metadata::Library;
use crate::services::launch::jvm_args::LaunchExecutor;
use crate::services::version::locator::DefaultVersionLocator;
use crate::util::file_helper::unzip;
use crate::util::lib_helper::{check_libs_ver, is_natives, is_rule_suitable, maven_to_path};
use crate::util::platform::{get_arch, get_current_arch, get_current_os_name};

/// 启动执行器实现（源：`internal sealed class LaunchExecutor : ILaunchExecutor` 的
/// LaunchAsync / KillAsync 部分；struct 定义与参数组装方法在 jvm_args.rs）。
#[async_trait]
impl crate::api::launch::LaunchExecutor for LaunchExecutor {
    /// 以给定选项启动游戏进程（源：`LaunchAsync`）。
    ///
    /// 流程：覆盖游戏目录（GameRoot）→ 预热版本元数据缓存（源 meta 未使用，仅触发扫描）→
    /// 解压 natives → 组装参数（jvm_args.rs）→ spawn java 进程 → 后台任务读输出管道并等待
    /// 退出 → 立即返回 `LaunchResult{Success=true, ProcessId, Message="进程已启动: {pid}"}`。
    /// 失败（源 catch 块）：写入 `logs/launch-errors.log` 后以 `Err` 返回。
    async fn launch(&self, options: LaunchOptions) -> Result<LaunchResult, Error> {
        // 源：var originalGameDir = _gameDir;（赋值后从未使用，保留以对齐行为；
        // 覆盖语义见 effective_game_dir）
        let _original_game_dir = &self.game_dir;

        // 源：var locator = new DefaultVersionLocator(_gameDir);
        //      var meta = locator.GetVersionMetadata(launchOptions.Version);
        // meta 在 LaunchAsync 中未被使用（仅预热 locator 缓存），此处保留调用以对齐行为。
        // 该调用在源中位于 try 之外（异常会直接使任务故障），Rust 侧 locator 构造与
        // get_version_metadata 均无错误路径（B6 已定），故无 Err 分支。
        {
            let locator = DefaultVersionLocator::new(
                self.effective_game_dir(&options),
                DownloadMirror::Official,
            );
            let _meta = locator.get_version_metadata(&options.version);
        }

        // 源 try { ... } catch (Exception ex) { 写日志; 返回失败 LaunchResult }：
        // Rust 以 Err(Error) 表达失败，日志写入保留（尽力而为）
        match self.launch_inner(&options).await {
            Ok(result) => Ok(result),
            Err(err) => {
                log_launch_error(&err);
                Err(err)
            }
        }
    }

    /// 按进程 ID 结束进程，返回是否成功结束（源：`KillAsync`）。
    ///
    /// 源语义：进程不存在（ArgumentException）或其他异常 → false，从不抛异常 → Rust 恒 Ok。
    /// Windows：`taskkill /PID {pid} /T /F`（对应 `Kill(true)` 杀整个进程树），找不到进程时
    /// taskkill 返回非零退出码 → false（对应 ArgumentException 分支）。
    /// Unix：`kill -0` 探测进程存在（对应 GetProcessById 抛异常）→ 递归收集子进程树
    /// （`ps -eo pid=,ppid=` 解析，从叶子到根 `kill -9`）→ 最后杀主进程。
    /// 进程树 kill 用子进程递归实现（Android toybox 的 ps 支持 -eo 输出格式）。
    async fn kill(&self, process_id: i32) -> Result<bool, Error> {
        let pid = process_id.to_string();
        let succeeded = if cfg!(windows) {
            tokio::process::Command::new("taskkill")
                .args(["/PID", pid.as_str(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            // 先探测进程是否存在（源 Process.GetProcessById 不存在时抛 ArgumentException → false）
            let probe = tokio::process::Command::new("kill")
                .args(["-0", pid.as_str()])
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if !probe {
                return Ok(false);
            }
            // 收集进程树（pid -> ppid），从叶子到根依次 kill -9
            let descendants = collect_descendants(process_id).await;
            let mut all_killed = true;
            for child in &descendants {
                let ok = tokio::process::Command::new("kill")
                    .args(["-9", &child.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                all_killed &= ok;
            }
            let main_killed = tokio::process::Command::new("kill")
                .args(["-9", pid.as_str()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            all_killed && main_killed
        };
        Ok(succeeded)
    }
}

/// Unix 进程树收集：`ps -eo pid=,ppid=` 解析全部进程，BFS 收集目标进程的
/// 全部后代（含孙进程），返回从最深叶子到直接子进程的逆序列表。
/// 解析失败（ps 不可用/输出异常）返回空列表（仅杀主进程，保守降级）。
async fn collect_descendants(root_pid: i32) -> Vec<i32> {
    let out = tokio::process::Command::new("ps")
        .args(["-eo", "pid=,ppid="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();

    // pid -> Vec<child pid>
    let mut children: std::collections::HashMap<i32, Vec<i32>> = std::collections::HashMap::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        if let (Some(pid), Some(ppid)) = (it.next().and_then(|v| v.parse::<i32>().ok()), it.next().and_then(|v| v.parse::<i32>().ok())) {
            children.entry(ppid).or_default().push(pid);
        }
    }

    // BFS 收集全部后代
    let mut descendants = Vec::new();
    let mut queue: Vec<i32> = children.get(&root_pid).cloned().unwrap_or_default();
    // 收集后用拓扑（叶子优先）排序：按深度降序
    let mut with_depth: Vec<(i32, usize)> = Vec::new();
    let mut depth = 0usize;
    while !queue.is_empty() {
        let mut next = Vec::new();
        for pid in &queue {
            with_depth.push((*pid, depth));
            if let Some(grand) = children.get(pid) {
                next.extend(grand.iter().copied());
            }
        }
        queue = next;
        depth += 1;
    }
    // 按深度降序（叶子先杀）
    with_depth.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
    descendants.extend(with_depth.into_iter().map(|(pid, _)| pid));
    descendants
}

impl LaunchExecutor {
    /// 内部启动流程（源 LaunchAsync 的 try 块；异常经 ? 向上传播，由 launch 统一处理）。
    async fn launch_inner(&self, options: &LaunchOptions) -> Result<LaunchResult, Error> {
        let game_dir = self.effective_game_dir(options);

        // 解压 natives（源：UnzipNatives(launchOptions)）
        self.unzip_natives(options)?;

        // 拼接参数（源：SelectParams(launchOptions) —— jvm_args.rs 协同提供）
        let params_str = self.select_params(options)?;

        // 源：FileName = NormalizeArg(launchOptions.JavaOptions?.JavaPath ?? "java")
        let java_path = options
            .java_options
            .as_ref()
            .map(|j| j.java_path.as_str())
            .unwrap_or("java");
        let file_name = crate::services::launch::jvm_args::normalize_arg(java_path);
        // ⚠️ 偏差：normalize_arg 对含空格路径加外层引号；直接传给 Command::new 时，
        // Windows CreateProcess 会将带引号的 exe 路径解析失败（os error 123，
        // ERROR_INVALID_NAME），Unix execvp 亦不去引号 → 统一在此去引号（见日志 p34）
        let file_name = file_name.trim_matches('"').to_string();

        // 源：WorkingDirectory = VersionIsolation ? gameDir/versions/version : gameDir
        let working_dir = if options.version_isolation {
            Path::new(&game_dir)
                .join("versions")
                .join(&options.version)
        } else {
            Path::new(&game_dir).to_path_buf()
        };

        // 源：Console.Error.WriteLine($"> {NormalizeArg(...)} {paramsStr}")
        eprintln!("> {} {}", file_name, params_str);

        // 源：ProcessStartInfo{ UseShellExecute=false, RedirectStandardOutput=true,
        //      RedirectStandardError=true, CreateNoWindow=true } + process.Start()
        // 参数整串按 .NET/Windows 命令行解析规则切分（split_command_line）
        let mut cmd = tokio::process::Command::new(&file_name);
        cmd.args(split_command_line(&params_str))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&working_dir);
        // 源 CreateNoWindow=true：Windows 下不创建控制台窗口（CREATE_NO_WINDOW）
        // tokio::process::Command::creation_flags 为固有方法，无需导入 trait
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Params {
                message: format!("启动失败,{e}"),
                source: Some(Box::new(e)),
            })?;

        // 源：LaunchResult.ProcessId = process.Id
        let process_id = child.id().map(|id| id as i32).unwrap_or(0);

        // 输出管道处理（源：BeginOutputReadLine / BeginErrorReadLine 的读线程 + Exited 事件）：
        // 后台任务并行逐行读取 stdout/stderr（必须读管道，否则子进程输出缓冲满会阻塞），
        // 读完后等待进程退出并打印退出码。launch 立即返回，与源行为一致（源返回时进程刚启动）。
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        tokio::spawn(async move {
            let out_fut = OptionFuture::from(stdout_pipe.map(|p| forward_pipe(p, true)));
            let err_fut = OptionFuture::from(stderr_pipe.map(|p| forward_pipe(p, false)));
            let _ = futures::future::join(out_fut, err_fut).await;
            // 源：process.Exited → result.OnExit?.Invoke(process.ExitCode)（写 stderr）+ Dispose
            let mut child = child;
            match child.wait().await {
                Ok(status) => eprintln!("进程退出，代码: {}", status.code().unwrap_or(-1)),
                Err(e) => eprintln!("等待进程退出失败: {e}"),
            }
        });

        Ok(LaunchResult {
            success: true,
            process_id,
            message: Some(format!("进程已启动: {process_id}")),
        })
    }

    /// 有效游戏目录（源 LaunchAsync 开头对 `_gameDir` 的覆盖规则：
    /// `if (!string.IsNullOrEmpty(GameRoot)) _gameDir = GameRoot`）。
    /// ⚠️ UNMAPPED：源为实例字段持久修改（同一实例后续调用沿用覆盖值）；Rust `&self`
    /// 不可变，按每次调用独立计算（见日志 p34）。
    /// pub(crate)：jvm_args.rs 全部路径拼接统一经此解析，保证每实例 game_root 覆盖生效
    /// （与源 `if (!string.IsNullOrEmpty(GameRoot)) _gameDir = GameRoot` 的字段变异语义对齐）。
    pub(crate) fn effective_game_dir(&self, options: &LaunchOptions) -> String {
        match &options.game_root {
            Some(root) if !root.is_empty() => root.clone(),
            _ => self.game_dir.clone(),
        }
    }

    /// 获取 natives 库的下载相对路径（源：`GetNativePath`，private static）。
    /// 优先取 downloads.classifiers 中按当前 OS/架构（`${arch}` → "64"/"32"）匹配的 artifact
    /// path；否则回退 `MavenToPath(native.Name)`。
    pub(crate) fn get_native_path(&self, library: &Library) -> String {
        if let (Some(natives), Some(classifiers)) =
            (&library.natives, library.downloads.classifiers.as_ref())
        {
            let os_name = get_current_os_name();
            if let Some(classifier_template) = natives.get(os_name) {
                let key = classifier_template.replace("${arch}", get_current_arch());
                if let Some(artifact) = classifiers.get(&key) {
                    if !artifact.path.is_empty() {
                        return artifact.path.clone();
                    }
                }
            }
        }
        maven_to_path(&library.name)
    }

    /// 解压 natives 到版本隔离目录 + java.library.path 子目录（源：`UnzipNatives`）。
    ///
    /// 源签名返回 bool（恒 true，失败均经异常向上传播）；Rust 以 `Result<(), Error>` 表达
    /// 传播语义，成功为 `Ok(())`（等价恒 true）。Trace.WriteLine 日志 → eprintln!。
    pub(crate) fn unzip_natives(&self, options: &LaunchOptions) -> Result<(), Error> {
        let game_dir = self.effective_game_dir(options);
        let natives = self.get_natives(options);
        let natives_dir = Path::new(&game_dir)
            .join("versions")
            .join(&options.version)
            .join(format!("{}-natives", options.version))
            .to_string_lossy()
            .into_owned();

        if !natives.is_empty() {
            // 逐个解压 natives JAR 到 natives 目录（保留 JAR 内部目录结构）
            for native in &natives {
                let zip_file_path =
                    Path::new(&game_dir).join("libraries").join(self.get_native_path(native));
                if zip_file_path.is_file() {
                    unzip(zip_file_path.to_str().unwrap_or_default(), &natives_dir);
                    eprintln!("已解压Natives: {}", native.name);
                } else {
                    eprintln!("Natives文件不存在: {}", zip_file_path.to_string_lossy());
                }
            }
        } else {
            eprintln!("没有需要解压的Natives文件");
        }

        // 同时解压到版本 JSON 中 java.library.path 指定的子目录（如果有）
        let java_lib_dir = self.parse_java_library_path(options, &natives_dir)?;
        if !java_lib_dir.is_empty() && java_lib_dir != natives_dir {
            let java_lib_path = Path::new(&java_lib_dir);
            // 清空后重新解压，避免上次残留（如错误架构）的库因"不覆盖"逻辑而无法被正确库替换
            if java_lib_path.is_dir() {
                // 源：try { Directory.Delete(javaLibDir, true); } catch { }
                let _ = fs::remove_dir_all(java_lib_path);
            }
            // 源：Directory.CreateDirectory（失败抛异常向上传播）
            fs::create_dir_all(java_lib_path).map_err(io_err)?;
            for native in &natives {
                let zip_file_path =
                    Path::new(&game_dir).join("libraries").join(self.get_native_path(native));
                if zip_file_path.is_file() {
                    unzip(zip_file_path.to_str().unwrap_or_default(), &java_lib_dir);
                }
            }
            // 扁平化 java.library.path 子目录中的原生库文件
            // 源：Windows ? ".dll" : MacOS ? ".dylib" : ".so"
            let keep_ext = if cfg!(windows) {
                ".dll"
            } else if cfg!(target_os = "macos") {
                ".dylib"
            } else {
                ".so"
            };
            LaunchExecutor::flatten_natives(&java_lib_dir, keep_ext)?;
            eprintln!("已解压Natives到java.library.path子目录: {java_lib_dir}");
        }

        Ok(())
    }

    /// 判断目录名是否为"非当前主机架构"的架构目录（源：`IsForeignArchDir`，private static）。
    /// 新版 LWJGL natives jar 内按架构分目录打包（如 windows/x64、windows/arm64、windows/x86），
    /// 需要跳过非本机架构目录，否则扁平化时会把错误架构的库放入 java.library.path 导致无法加载。
    pub(crate) fn is_foreign_arch_dir(name: &str) -> bool {
        // 当前主机架构的所有别名（源 SystemHelper.GetArch() switch；忽略大小写）
        let host_aliases: &[&str] = match get_arch() {
            "x64" => &["x64", "x86_64", "x86-64", "amd64"],
            "arm64" => &["arm64", "aarch64"],
            "x86" => &["x86", "i386", "i686"],
            _ => &[],
        };
        // 所有已知架构目录名
        const KNOWN_ARCH: [&str; 11] = [
            "x64", "x86_64", "x86-64", "amd64", "arm64", "aarch64", "x86", "i386", "i686",
            "arm", "arm32",
        ];
        let is_known = KNOWN_ARCH.iter().any(|a| a.eq_ignore_ascii_case(name));
        let is_host = host_aliases.iter().any(|a| a.eq_ignore_ascii_case(name));
        // 是已知架构目录，但不属于当前主机架构 → 视为异构目录，需跳过
        is_known && !is_host
    }

    /// 将嵌套目录中的原生库文件（.so/.dll/.dylib）扁平化到其所在子目录的根
    /// （源：`FlattenNatives`，private static；IO 失败经异常传播 → Err）。
    pub(crate) fn flatten_natives(dir: &str, keep_ext: &str) -> Result<(), Error> {
        let dir_path = Path::new(dir);
        if !dir_path.is_dir() {
            return Ok(());
        }

        // 递归遍历所有子目录（跳过非当前主机架构的目录，避免错误架构的库覆盖正确架构）
        let sub_dirs = read_sub_dirs(dir_path)?;
        for sub_dir in &sub_dirs {
            if LaunchExecutor::is_foreign_arch_dir(file_name(sub_dir)) {
                continue;
            }
            LaunchExecutor::flatten_natives(sub_dir, keep_ext)?;
        }

        // 将当前目录子目录中的原生库文件移动到当前目录
        // （重新列目录：递归可能已删除被清空的子目录，与 C# 两轮 Directory.GetDirectories 一致）
        let sub_dirs = read_sub_dirs(dir_path)?;
        for sub_dir in &sub_dirs {
            if LaunchExecutor::is_foreign_arch_dir(file_name(sub_dir)) {
                continue;
            }
            let sub_path = Path::new(sub_dir);
            // 源：Directory.GetFiles(subDir)——仅文件
            for entry in fs::read_dir(sub_path).map_err(io_err)? {
                let entry = entry.map_err(io_err)?;
                let file_path = entry.path();
                if !file_path.is_file() {
                    continue;
                }
                // 源：Path.GetExtension（含前导点，如 ".dll"）
                let ext = file_path
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                if ext.eq_ignore_ascii_case(keep_ext) {
                    let dest_path =
                        dir_path.join(file_path.file_name().unwrap_or_default());
                    // 源：File.Move（目标已存在则不移动，不覆盖）
                    if !dest_path.exists() {
                        fs::rename(&file_path, &dest_path).map_err(io_err)?;
                    }
                }
            }
            // 如果子目录为空则删除
            // 源：try { Directory.Delete(subDir); } catch { }（失败吞掉）
            let is_empty = sub_path
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false);
            if is_empty {
                let _ = fs::remove_dir(sub_path);
            }
        }
        Ok(())
    }

    /// 从 JVM 参数中解析 java.library.path（源：`ParseJavaLibraryPath`，private）。
    /// 遍历 GetJVMParams 结果，取首个含 "java.library.path"（忽略大小写）的参数，
    /// 截取首个 '=' 之后并 trim，替换全部 `${natives_directory}`；
    /// 非空且 != nativesDir 时返回，否则返回空串。
    pub(crate) fn parse_java_library_path(
        &self,
        options: &LaunchOptions,
        natives_dir: &str,
    ) -> Result<String, Error> {
        // 源：GetJVMParams(options)（jvm_args.rs 协同提供；异常向上传播 → ?）
        let jvms = self.get_jvm_params(options)?;
        for jvm in &jvms {
            // 源：Contains("java.library.path", OrdinalIgnoreCase)
            // ⚠️ 偏差：to_lowercase 为 Unicode 小写化，OrdinalIgnoreCase 为 ASCII 忽略大小写；
            // 参数均为 ASCII，实际等价（见日志 p34）
            if jvm.to_lowercase().contains("java.library.path") {
                if let Some(eq_idx) = jvm.find('=') {
                    let mut lib_path = jvm[eq_idx + 1..].trim().to_string();
                    lib_path = lib_path.replace("${natives_directory}", natives_dir);
                    // verbatim（\\?\ 前缀）路径下 '/' 不是分隔符而是普通字符，与模板
                    // 里的 '/java' 拼接后 CreateDirectory 报 ERROR_INVALID_NAME (123)；
                    // Windows 上 '/' 与 '\' 等价（verbatim 除外），统一换成 '\' 无害且必要。
                    if lib_path.starts_with(r"\\?\") {
                        lib_path = lib_path.replace('/', "\\");
                    }
                    if !lib_path.is_empty() && lib_path != natives_dir {
                        return Ok(lib_path);
                    }
                }
            }
        }
        Ok(String::new())
    }

    /// 收集适用于当前平台的 natives 库列表（源：`GetNatives`，private，递归处理继承）。
    /// 有 rules 时逐条 is_rule_suitable（单规则判定，非 IsRulesSuitable），无 rules 走 else
    /// 分支；命中且不重复则加入（⚠️ 偏差：C# List.Contains 为引用相等，Rust 为值相等，
    /// 去重更激进，见日志 p34）。继承版本递归后整体 check_libs_ver 去重取最新。
    pub(crate) fn get_natives(&self, options: &LaunchOptions) -> Vec<Library> {
        let game_dir = self.effective_game_dir(options);
        let mut lib_list: Vec<Library> = Vec::new();

        // 源：var locator = new DefaultVersionLocator(_gameDir);
        //      var meta = locator.GetVersionMetadata(options.Version);
        let locator = DefaultVersionLocator::new(game_dir, DownloadMirror::Official);
        if let Some(meta) = locator.get_version_metadata(&options.version) {
            // 源：if (meta?.Libraries is not null)（Rust 模型 libraries 为必填 Vec）
            for lib in &meta.libraries {
                if let Some(rules) = &lib.rules {
                    if !rules.is_empty() {
                        for rule in rules {
                            if is_rule_suitable(Some(rule)) && is_natives(lib) {
                                if !lib_list.contains(lib) {
                                    lib_list.push(lib.clone());
                                }
                            }
                        }
                    }
                } else if is_natives(lib) {
                    if !lib_list.contains(lib) {
                        lib_list.push(lib.clone());
                    }
                }
            }

            // 源：if (meta is not null && !string.IsNullOrEmpty(meta.InheritsFrom))
            //      LibList.AddRange(GetNatives(options with { Version = meta.InheritsFrom }));
            if let Some(inherits) = meta.inherits_from.as_deref().filter(|s| !s.is_empty()) {
                let inherited_options = LaunchOptions {
                    version: inherits.to_string(),
                    ..options.clone()
                };
                lib_list.append(&mut self.get_natives(&inherited_options));
            }
        }

        check_libs_ver(lib_list)
    }

    // 注：parse_game_json_config（B8 重复版）已删除——版本 JSON 解析统一走 jvm_args.rs 的 parse_game_json
    // 原实现语义（源 ParseGameJson）：文件缺失/畸形 JSON 的错误映射见 jvm_args.rs 对应方法
}

// ── 源 private static / 辅助函数 ────────────────────────

/// 逐行读取子进程管道并转发到控制台（源 BeginOutputReadLine / BeginErrorReadLine 的
/// 行读取线程语义；空行丢弃，对应源 `if (!string.IsNullOrEmpty(e.Data))`；
/// 输出流向：源 OnOutput → Console.WriteLine("[OUT] ...")，OnError → Console.Error.WriteLine）
async fn forward_pipe<R>(mut pipe: R, is_stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = tokio::io::BufReader::new(&mut pipe);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            // 0 = EOF；读错误同样结束（源读线程异常不向外传播）
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if !trimmed.is_empty() {
                    if is_stdout {
                        println!("[OUT] {trimmed}");
                    } else {
                        eprintln!("[ERR] {trimmed}");
                    }
                }
            }
        }
    }
}

/// 将 .NET 命令行参数字符串切分为参数数组。
/// 源 `ProcessStartInfo.Arguments` 为整串，由 CRT（Windows）/.NET（Unix）按
/// "空白分隔 + 双引号分组（引号不保留）"规则解析；Rust Command 接收参数数组。
/// 该字符串由 NormalizeArg 产出（含空格值加双引号），本切分覆盖其合法形态。
/// ⚠️ 偏差：CommandLineToArgvW 的 `\"` 转义、`""` 空参数（被丢弃）未复刻（见日志 p34）。
/// 命令行切分（对应 Windows CommandLineToArgvW 规则；源为
/// `ProcessStartInfo.Arguments` 整串 + CommandLineToArgvW）：
/// - 双引号切换引号状态；引号内空白不切分
/// - 反斜杠转义：`\` + `"` 时偶数个 `\` → 半数 `\` + 引号开关；奇数个 `\` → 半数 `\` + 字面 `"`
/// - `""` 空参数保留为空字符串项（any_content 标记）
/// pub(crate)：launch 与 install（installer.rs::run_install_process 修复 windows java 处理器
/// 整串单参问题）共用同一套切词，避免两处规则漂移。
pub(crate) fn split_command_line(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut any_content = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut count = 1;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    count += 1;
                }
                if chars.peek() == Some(&'"') {
                    current.push_str(&"\\".repeat(count / 2));
                    if count % 2 == 1 {
                        current.push('"');
                    } else {
                        chars.next();
                        in_quotes = !in_quotes;
                    }
                    any_content = true;
                } else {
                    current.push_str(&"\\".repeat(count));
                    any_content = true;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                any_content = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if any_content {
                    args.push(std::mem::take(&mut current));
                    any_content = false;
                }
            }
            c => {
                current.push(c);
                any_content = true;
            }
        }
    }
    if any_content {
        args.push(current);
    }
    args
}

/// 源 catch 块：写入 `logs/launch-errors.log`（尽力而为，失败静默）。
/// 记录格式 `[{UTC:O}] [LaunchExecutor] {ex}\n\n`；时间戳为秒级 UTC（近似，见 unix_to_utc_string）。
fn log_launch_error(err: &Error) {
    // 源：Directory.CreateDirectory(logDir) + File.AppendAllText（catch 内，失败静默）
    let log_dir = Path::new(&get_data_dir()).join("logs");
    let _ = fs::create_dir_all(&log_dir);
    let log_file = log_dir.join("launch-errors.log");
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(log_file) {
        Ok(f) => f,
        Err(_) => return,
    };
    let _ = writeln!(
        file,
        "[{}] [LaunchExecutor] {err}\n",
        unix_to_utc_string(SystemTime::now())
    );
}

/// 数据目录（源 private static GetDataDir）：QOMICEX_HOME 环境变量（非空优先）→
/// LocalApplicationData/qomicex-launcher；若存在 `.qomicex-bootstrap` 文件，读其内容
/// （trim 后非空）作为自定义目录。
fn get_data_dir() -> String {
    if let Ok(env) = std::env::var("QOMICEX_HOME") {
        if !env.is_empty() {
            return env;
        }
    }
    let default_dir = default_local_app_data().join("qomicex-launcher");
    let bootstrap_file = default_dir.join(".qomicex-bootstrap");
    if let Ok(content) = fs::read_to_string(&bootstrap_file) {
        let custom_dir = content.trim();
        if !custom_dir.is_empty() {
            return custom_dir.to_string();
        }
    }
    default_dir.to_string_lossy().into_owned()
}

/// LocalApplicationData（源 Environment.SpecialFolder.LocalApplicationData）：
/// Windows %LOCALAPPDATA%；macOS ~/Library/Application Support；
/// Linux $XDG_DATA_HOME 或 ~/.local/share；解析失败回退当前目录。
fn default_local_app_data() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Unix 秒 → UTC 时间串（近似源 `DateTime.UtcNow.ToString("O")`；⚠️ 无 chrono 依赖
/// （B2 定案），秒级精度、无小数秒/时区偏移，格式 `YYYY-MM-DDTHH:MM:SSZ`，见日志 p34）。
/// 民用日期换算采用 Howard Hinnant civil_from_days 算法。
fn unix_to_utc_string(t: SystemTime) -> String {
    let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    // 毫秒精度（源 {UTC:O} 为 7 位小数，日志场景毫秒足够；偏差记录见 p34）
    let millis = (dur.subsec_nanos() / 1_000_000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{sec:02}.{millis:03}Z")
}

/// 列出目录下的子目录（完整路径，源 `Directory.GetDirectories`；IO 失败 → Err）
fn read_sub_dirs(dir: &Path) -> Result<Vec<String>, Error> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        if entry.path().is_dir() {
            dirs.push(entry.path().to_string_lossy().into_owned());
        }
    }
    Ok(dirs)
}

/// 取路径末段文件名（源 `Path.GetFileName`）
fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
}

/// 读取顶层字符串字段（源 STJ 语义：缺失/显式 null → null；类型不符 → JsonException）

/// 源 ParamsException("版本Json解析失败")（逐字消息）

/// 启动流程中的 IO 失败 → Error::Params（消息保留源失败结果模式 "启动失败,{ex.Message}"，
/// 见日志 p34 错误映射决策）
fn io_err(e: std::io::Error) -> Error {
    Error::Params {
        message: format!("启动失败,{e}"),
        source: Some(Box::new(e)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_line_basic() {
        assert_eq!(
            split_command_line("-Xmx2G -jar game.jar"),
            vec!["-Xmx2G", "-jar", "game.jar"]
        );
    }

    #[test]
    fn split_command_line_quoted_spaces() {
        assert_eq!(
            split_command_line(r#""C:\Program Files\Java\jdk-17\bin\java.exe" -jar x.jar"#),
            vec![r#"C:\Program Files\Java\jdk-17\bin\java.exe"#, "-jar", "x.jar"]
        );
    }

    #[test]
    fn split_command_line_backslash_quote_escape() {
        // 奇数个反斜杠 + 引号 → 字面引号（不切换状态）
        assert_eq!(split_command_line(r#"a\""#), vec![r#"a""#]);
        // 偶数个反斜杠 + 引号 → 半数反斜杠 + 引号开关
        assert_eq!(split_command_line(r#"a\\"b c""#), vec!["a\\b c"]);
    }

    #[test]
    fn split_command_line_empty_args() {
        assert_eq!(split_command_line(r#""""#), vec![""]);
        assert_eq!(split_command_line(r#"a "" b"#), vec!["a", "", "b"]);
    }

    #[test]
    fn utc_timestamp_has_millis() {
        let s = unix_to_utc_string(std::time::SystemTime::now());
        // yyyy-MM-ddTHH:mm:ss.SSSZ 形态
        assert!(s.len() >= 24);
        assert!(s.ends_with('Z'));
        assert!(s.contains('.'));
    }
}



