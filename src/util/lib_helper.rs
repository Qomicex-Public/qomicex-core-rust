//! 库辅助工具
//! 对应源：Qomicex.Core.AOT/Utils/LibHelper.cs（276 行）
//! 功能：库坐标解析（Maven group:artifact:version[:classifier[:type]] → 路径）、
//! classpath/natives 判定、规则（Rule）平台适配判定、库冲突去重与版本排序。

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::models::version_metadata::{Library, Rule};
use crate::util::platform::is_os_match;

/// 判断库是否属于 classpath（对应 LibHelper.IsClassPath）
/// C# 逻辑：Downloads 非空时看 Artifact 是否存在；Downloads 缺失时，Natives 为空则视为 classpath
/// ⚠️ 偏差：Rust 模型（B1）中 Library.downloads 为必填字段，无法表达"C# 中 Downloads 为 null"的形态；
/// 此处以"artifact 与 classifiers 均为空"近似"无下载信息"，再按 C# 的 Natives 分支判定。
pub fn is_class_path(library: &Library) -> bool {
    // C# 分支1：Downloads 存在且 Artifact 存在 → classpath
    if library.downloads.artifact.is_some() {
        return true;
    }
    // C# 分支2（Downloads 为 null 时）：Natives 为 null → classpath
    if library.downloads.classifiers.is_none() && library.natives.is_none() {
        return true;
    }
    false
}

/// 判断库是否为 natives 库（对应 LibHelper.IsNatives）
/// 满足任一条件即视为 natives：natives 映射非空 / downloads.classifiers 非空 / 名称含 "natives"（忽略大小写）
pub fn is_natives(library: &Library) -> bool {
    if library.natives.is_some() {
        return true;
    }
    if library.downloads.classifiers.is_some() {
        return true;
    }
    if library.name.to_lowercase().contains("natives") {
        return true;
    }
    false
}

/// 判断单条规则是否适合当前系统（对应 LibHelper.IsRuleSuitable）
/// 规则为 null → true；action=allow：os 缺失 → true，否则取 os 匹配结果；
/// action=disallow：恒为 false（os 匹配 → false，os 缺失 → false，其他情形落到最终 false）
/// 说明：C# 要求 Os.Name 非 null；Rust 模型中 name 为必填 String、os 为 Option，等价于 os 非空判断
pub fn is_rule_suitable(rule: Option<&Rule>) -> bool {
    let Some(rule) = rule else {
        return true;
    };

    if rule.action == "allow" {
        if let Some(os) = &rule.os {
            return is_os_match(os);
        }
        return true;
    } else if rule.action == "disallow" {
        if let Some(os) = &rule.os {
            if is_os_match(os) {
                return false;
            }
        } else {
            return false;
        }
    }
    false
}

/// 判断规则列表是否允许（对应 LibHelper.IsRulesSuitable）
/// 逐个应用规则：allow 且（os 缺失或匹配）→ 允许；disallow 且（os 缺失或匹配）→ 拒绝；返回最终状态
pub fn is_rules_suitable(rules: &[Rule]) -> bool {
    let os_or_match = |rule: &Rule| -> bool {
        match &rule.os {
            Some(os) => is_os_match(os),
            None => true,
        }
    };

    let mut allow = false;
    for rule in rules {
        if rule.action == "allow" {
            if os_or_match(rule) {
                allow = true;
            }
        } else if rule.action == "disallow" {
            if os_or_match(rule) {
                allow = false;
            }
        }
    }
    allow
}

/// 按分组键去重并取组内版本最高的库（对应 LibHelper.CheckLibsVer）
/// 分组键为 group:artifact[:classifier]（见 get_lib_group_key）；保持每组首次出现的顺序（同 C# GroupBy）
pub fn check_libs_ver(libs: Vec<Library>) -> Vec<Library> {
    let mut best: Vec<(String, Library)> = Vec::new();
    for lib in libs {
        let key = get_lib_group_key(&lib.name);
        match best.iter_mut().find(|(k, _)| *k == key) {
            Some((_, newest)) => {
                if version_sort_integer(&get_lib_version(&lib), &get_lib_version(newest)) > 0 {
                    *newest = lib; // 替换为更新版本
                }
            }
            None => best.push((key, lib)),
        }
    }
    best.into_iter().map(|(_, lib)| lib).collect()
}

/// 移除 -all 聚合库与其独立拆分库之间的版本冲突（对应 LibHelper.RemoveConflictingLibraries）
/// 规则：对 artifactId 以 "-all" 结尾的库，若存在同 groupId 的对应独立库（baseCoord: 前缀），
/// 且独立库版本更高，则移除聚合库。C# 经 Trace.WriteLine 输出日志，Rust 使用 eprintln!。
pub fn remove_conflicting_libraries(libs: Vec<Library>) -> Vec<Library> {
    // C# HashSet(StringComparer.OrdinalIgnoreCase) → Rust 的 HashSet 区分大小写，
    // 键为原始名称、判等时用 ASCII 忽略大小写（Maven 坐标均为 ASCII，语义等价）
    let mut to_remove: HashSet<String> = HashSet::new();

    for lib in &libs {
        let parts: Vec<&str> = lib.name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }

        let artifact_id = parts[1];
        if !artifact_id.to_lowercase().ends_with("-all") {
            continue;
        }

        let group_id = parts[0];
        let base_name = &artifact_id[..artifact_id.len() - 4];
        let base_coord = format!("{group_id}:{base_name}");
        let prefix = format!("{base_coord}:");

        let individual_lib = libs.iter().find(|l| {
            l.name
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
                && !l.name.eq_ignore_ascii_case(&lib.name)
        });

        let Some(individual_lib) = individual_lib else {
            continue;
        };

        let fat_version = parts[2];
        let ind_version = get_lib_version(individual_lib);
        if version_sort_integer(fat_version, &ind_version) < 0 {
            to_remove.insert(lib.name.clone());
            eprintln!(
                "[QML] 移除冲突库 {}（已被 {} 取代）",
                lib.name, individual_lib.name
            );
        }
    }

    if to_remove.is_empty() {
        return libs;
    }
    libs.into_iter()
        .filter(|l| !to_remove.contains(&l.name))
        .collect()
}

/// 从库名称中提取版本号（对应 LibHelper.GetLibVersion）
/// 名称按 ':' 分割取第 3 段（index 2）；不足 3 段或名称为空返回空字符串
pub fn get_lib_version(library: &Library) -> String {
    let full_name = &library.name;
    if full_name.is_empty() {
        return String::new();
    }

    let temp: Vec<&str> = full_name.split(':').collect();
    if temp.len() >= 3 {
        return temp[2].to_string();
    }
    String::new()
}

/// 库分组键（对应 LibHelper.GetLibGroupKey，private）：group:artifact[:classifier]
fn get_lib_group_key(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return name.to_string();
    }
    let mut key = format!("{}:{}", parts[0], parts[1]);
    if parts.len() >= 4 {
        key.push(':');
        key.push_str(parts[3]);
    }
    key
}

/// 版本号比较（对应 LibHelper.VersionSortInteger，private），返回 -1/0/1（符号语义，同 C# 调用处）
/// 将版本串小写后按"字母串|数字串"（正则 [a-z]+|[0-9]+）分词逐位比较：
/// - 数字按数值比较，字母串按序数比较；
/// - 特殊标签 pre/snapshot → -3、rc → -2、experimental → -4，可解析为负数参与数值比较；
/// - 任一侧 token 耗尽以 "-1" 补位；全部 token 相同后回退为整串序数比较（对应 C# string.Compare(Ordinal)）。
fn version_sort_integer(left: &str, right: &str) -> i32 {
    let left = left.to_lowercase();
    let right = right.to_lowercase();

    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    let token_re = TOKEN_RE.get_or_init(|| Regex::new(r"[a-z]+|[0-9]+").expect("静态正则编译失败"));

    let left_parts: Vec<String> = token_re
        .find_iter(&left)
        .map(|m| m.as_str().to_string())
        .collect();
    let right_parts: Vec<String> = token_re
        .find_iter(&right)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut i = 0;
    loop {
        if i >= left_parts.len() && i >= right_parts.len() {
            return string_compare_ordinal(&left, &right);
        }

        let l_val = left_parts
            .get(i)
            .cloned()
            .unwrap_or_else(|| "-1".to_string());
        let r_val = right_parts
            .get(i)
            .cloned()
            .unwrap_or_else(|| "-1".to_string());

        if l_val == r_val {
            i += 1;
            continue;
        }

        let l_val = convert_special_label(&l_val);
        let r_val = convert_special_label(&r_val);

        match (l_val.parse::<i64>(), r_val.parse::<i64>()) {
            (Ok(l_num), Ok(r_num)) => {
                if l_num > r_num {
                    return 1;
                }
                if l_num < r_num {
                    return -1;
                }
                i += 1; // 数值相等，继续下一 token
            }
            _ => return string_compare_ordinal(&l_val, &r_val),
        }
    }
}

/// 特殊版本标签转换（对应 LibHelper.ConvertSpecialLabel，private）
/// pre/snapshot → "-3"；rc → "-2"；experimental → "-4"；其余原样返回
fn convert_special_label(label: &str) -> String {
    match label {
        "pre" | "snapshot" => "-3".to_string(),
        "rc" => "-2".to_string(),
        "experimental" => "-4".to_string(),
        _ => label.to_string(),
    }
}

/// 移除字符串中的可选后缀（对应 LibHelper.RemoveOptionalSuffix，private）：截断首个 '@' 之前
fn remove_optional_suffix(input: &str) -> &str {
    match input.find('@') {
        Some(idx) => &input[..idx],
        None => input,
    }
}

/// Maven 坐标转文件路径（对应 LibHelper.MavenToPath）
/// 支持格式 group:artifact:version[:classifier[:type]]，另兼容 classifier@type 与 version@扩展名 写法；
/// 非法输入（空 / 少于 3 段 / 含空值）记录日志并返回空字符串。
pub fn maven_to_path(maven: &str) -> String {
    // 防御性检查：坐标为空直接返回
    if maven.trim().is_empty() {
        eprintln!("Maven坐标为空，无法转换路径");
        return String::new();
    }

    // 分割坐标（支持格式：group:artifact:version[:classifier[:type]]）
    let parts: Vec<&str> = maven.split(':').collect();

    // 最少需要3个部分（group:artifact:version）
    if parts.len() < 3 {
        eprintln!("无效的Maven坐标格式：{maven}，至少需要3个部分（group:artifact:version）");
        return String::new();
    }

    // 提取基础部分（确保不越界）
    let group = remove_optional_suffix(parts[0].trim());
    let artifact = remove_optional_suffix(parts[1].trim());
    let mut version = parts[2].trim().to_string();

    // 处理可选的classifier和type，兼容 classifier@type 与 classifier:type 两种格式
    let mut classifier = String::new();
    let mut r#type = "jar".to_string();

    // 处理版本号尾部的 @扩展名（如 1.16.5-20210115.111550@zip）
    if let Some((ver, ext)) = version.split_once('@') {
        let ver = ver.trim().to_string();
        if !ext.trim().is_empty() {
            r#type = ext.trim().to_string();
        }
        version = ver;
    }

    if parts.len() >= 4 {
        let classifier_part = parts[3].trim();
        if classifier_part.contains('@') {
            let (classifier_name, classifier_ext) = classifier_part
                .split_once('@')
                .expect("contains('@') 已保证 split_once 成功");
            classifier = classifier_name.trim().to_string();
            if !classifier_ext.trim().is_empty() {
                r#type = classifier_ext.trim().to_string();
            }
        } else {
            classifier = classifier_part.to_string();
            if parts.len() >= 5 && !parts[4].trim().is_empty() {
                r#type = parts[4].trim().to_string();
            }
        }
    }

    // 验证基础部分有效性
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        eprintln!("Maven坐标包含空值：{maven}");
        return String::new();
    }

    // 转换 group为路径（com.mumfrey → com/mumfrey）
    let group_path = group.replace('.', "/");

    // 构建文件名（artifact-version[-classifier].type）
    let mut file_name = format!("{artifact}-{version}");
    if !classifier.is_empty() {
        file_name.push_str(&format!("-{classifier}"));
    }
    file_name.push('.');
    file_name.push_str(&r#type);

    // 组合完整路径
    format!("{group_path}/{artifact}/{version}/{file_name}")
}

/// C# string.Compare(left, right, StringComparison.Ordinal) 的符号等价物（返回 -1/0/1）
/// 说明：Rust str 比较按 UTF-8 字节序，C# 按 UTF-16 码元序；版本串均为 ASCII，语义等价
fn string_compare_ordinal(left: &str, right: &str) -> i32 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
