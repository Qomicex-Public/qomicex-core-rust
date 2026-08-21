//! 版本识别工具（B2）
//! 对应源：Qomicex.Core.AOT/Utils/GameVersionHelper.cs（322 行）
//! 特殊兼容点：Java .class 文件常量池解析（高风险兼容点）——
//! 源用 BinaryReader 按大端序读取字节流，逐 tag 分支跳过/提取；
//! Rust 侧用原始字节切片 + 游标手工解析（u16/u32 大端 be），
//! 源定义的 17 种 CONSTANT_* tag 全部分支保留（含 LONG/DOUBLE
//! 占用双常量池索引、未知 tag 抛错语义），未做任何简化或省略。

use regex::Regex;
use serde_json::Value;
use sha1::Digest as _;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

const CONSTANT_UTF8: u8 = 1;
const CONSTANT_INTEGER: u8 = 3;
const CONSTANT_FLOAT: u8 = 4;
const CONSTANT_LONG: u8 = 5;
const CONSTANT_DOUBLE: u8 = 6;
const CONSTANT_CLASS: u8 = 7;
const CONSTANT_STRING: u8 = 8;
const CONSTANT_FIELDREF: u8 = 9;
const CONSTANT_METHODREF: u8 = 10;
const CONSTANT_INTERFACEMETHODREF: u8 = 11;
const CONSTANT_NAMEANDTYPE: u8 = 12;
const CONSTANT_METHODHANDLE: u8 = 15;
const CONSTANT_METHODTYPE: u8 = 16;
const CONSTANT_DYNAMIC: u8 = 17;
const CONSTANT_INVOKEDYNAMIC: u8 = 18;
const CONSTANT_MODULE: u8 = 19;
const CONSTANT_PACKAGE: u8 = 20;

/// 已知 JAR SHA1 哈希 → 版本号映射表（源 KNOWN_VERSIONS，84 条全部保留）
fn known_version(hash: &str) -> Option<&'static str> {
    match hash {
        "4df7880d26414b400640f0b8e54344df2b66c51a" => Some("1.0.0-rc1"),
        "9e04e60eef3fb4657b406dcb3ad5e3a675ecf6af" => Some("1.0.0-rc2-1"),
        "6a6b67d34149afc47cf9608b3967582639097df9" => Some("1.0.0-rc2-2"),
        "6e54fbe19b7797f3e3a2cb9feb5da41a40926db8" => Some("1.0.0-rc2-3"),
        "fe189e91a3e7166d46fad8ce53ba0ce34b4c5f97" => Some("a1.0.5"),
        "73f569bf5556580979606049204835ae1a54f04d" => Some("a1.0.5_01"),
        "e5838277b3bb193e58408713f1fc6e005c5f3c0c" => Some("a1.0.4"),
        "31e9736457ef3e0bfea69c720137a1bd8ba7caae" => Some("a1.0.3"),
        "4f9ce27cfc6394af533fde11a90b6a233dd908bf" => Some("a1.0.2_02"),
        "7457e763ad81eee1e63628d628647f53806dab7c" => Some("a1.0.2_01"),
        "02c57723da508aab36455782904bfd6e3e1023e6" => Some("a1.0.1_01"),
        "88c1931650b0e5be349017e124a7785a745111e9" => Some("inf-20100630-2"),
        "121fff417950ad72005ca4d882ca6269e874547b" => Some("inf-20100630-1"),
        "eb50bce3cb542488b3039aa0f4c3c0ec7595ab24" => Some("inf-20100629"),
        "4d31259a71c5886b987b9eca6034ca5552079eed" => Some("inf-20100627"),
        "d9fc6416186e1454945ab135f37c730c7d2c1adc" => Some("inf-20100625-2"),
        "990b531a26ae8e475032915938763c12cdb2dcf9" => Some("inf-20100625-1"),
        "644c050e846035e06a6637bffa2afee1e5769c8c" => Some("inf-20100624"),
        "d3eb1dce5a6c86dd0d6483ba56223276dcf32c30" => Some("inf-20100617-3"),
        "06641eca013fe5032a5f1a9d1289599f0970a735" => Some("inf-20100617-2"),
        "89eab2c1a353707cc00f074dffba9cb7a4f5e304" => Some("inf-20100618"),
        "47518a623da068728b50b4b53436dea4621b7bf8" => Some("inf-20100615"),
        "421318a554f17463a56a271d08e9597941d066d9" => Some("inf-20100611"),
        "a9efb36c142bf835d3d410150856dc9ceeaae81b" => Some("inf-20100608"),
        "7bbf38d53dd47753af266be4e1c5865342a26974" => Some("inf-20100607"),
        "27010a5137abd2c8d8df85e99c14f5406ec197b3" => Some("inf-20100420"),
        "a91c9d8e0184eda610213b1a5425fbfa078cb191" => Some("inf-20100415"),
        "86dd3b1558352b38d4d15c7ec51b9131bd7aed4b" => Some("inf-20100414"),
        "7b39167f14d9f0ce7af6819433856be7b82d2412" => Some("inf-20100413"),
        "a74c8ee1ecd57999e242952697bbde6cc0904f99" => Some("inf-20100330"),
        "47b1b32430a211520993552ba0a5e00c1af44724" => Some("inf-20100327"),
        "99da3b55b4db292faca59824e3ec76bf53a7eae6" => Some("inf-20100325"),
        "2c89471a81858d37ab0b01e042131878b6853b38" => Some("inf-20100321"),
        "7f1c48fc6d61dd0cbfd41b84fb0b0a22944aa02c" => Some("inf-20100320"),
        "ad7b3cd706098ac05c7dba61dacb40bafcd47db6" => Some("inf-20100316"),
        "65a00a10001978538ab8eef1a2533f47d4ecbe23" => Some("inf-20100313"),
        "801ce486bb7fd1b43a56bc5d226dfb1370c08678" => Some("in-20100223"),
        "af3d7f95ca75e130a9c5c74be0a9c09600a15686" => Some("in-20100219"),
        "2ba9e9a2bdac1e8af6a36819e9bb01375889b078" => Some("in-20100218"),
        "dcbe38d0e4ac2caec7e5c0f9ebcb0ec9179dcdff" => Some("in-20100214-2"),
        "e6bb9306dab60626ba6ffd24fc9742fd272f5acb" => Some("in-20100214-1"),
        "f1ae7e37e52b33753b35402e581eb65dc5bba877" => Some("in-20100212-2"),
        "5275aaf68d6388ef8278b575e95ae83ad641fe3e" => Some("in-20100212-1"),
        "fa8525be5612d00f6001be7d4cdb764b66e88f9d" => Some("in-20100207-2"),
        "054e3d3f4e2c0463f80aa323767e018e6c23c1cd" => Some("in-20100207-1"),
        "049b002cdd164e5c5e9b78780b12ab4dc2e80120" => Some("in-20100206-2103"),
        "b2abb22e001abf01ca7555ced5d6024350955d70" => Some("in-20100203"),
        "38d4df5132077ac60f0bdf67564f5fff4ee309e2" => Some("in-20100201-3"),
        "1f2ca31fc761207bcabc07f0cf4b725a9a3286e4" => Some("in-20100201-2"),
        "c871e820d5356b88b3ad854789162f8b9227c80c" => Some("in-20100130"),
        "03b858d31c090b629f406aa1d548ac7b25341f02" => Some("in-20100129"),
        "3f2418f906d438b26ae6c9dbbadf3942f5845504" => Some("in-20100128-2304"),
        "baf0c7b1e231f0984e1c35e27f38eea2743f8ee2" => Some("in-20100125-2"),
        "2cd03bcfc26c95bcf31b5d5e1d4dda7dc071ca6a" => Some("in-20100125-1"),
        "a0b58472ebf12f7e562b09b8a51dcb4cacc57005" => Some("in-20100111-1"),
        "38958105bfe0f7064b3c4996905cb6978d4d4b0b" => Some("in-20100105"),
        "3161652a6835c61817fda6fe13245c57528ed418" => Some("in-20091231-2"),
        "94ee2e7aa7d093fa8dfc684baa8bd8afe002580f" => Some("in-20091223-2"),
        "54622801f5ef1bcc1549a842c5b04cb5d5583005" => Some("c0.30_01c"),
        "51bc951530207b538596941a6f353f87dfc24233" => Some("c0.30-2"),
        "619ea74c6d0ae5c0125d1e31e299105e100139ab" => Some("c0.30-1"),
        "6a6f92b691f9d6b7ca991a6db8a1cfc6e319815b" => Some("c0.29_02"),
        "bb5e7f1c231f45fd630f30a75570937c103f5b55" => Some("c0.29_01"),
        "7ccde270abacd028d3618be99537ccf7071a605b" => Some("c0.28_01"),
        "aff4060249dd6152012218e120d7aad5e758de83" => Some("c0.27_st"),
        "349630cb1b895335c38b499f84dc28d9f8a38513" => Some("c0.25_05_st"),
        "0b387d2087edda894fae4af00de5ac202dbffa7c" => Some("c0.24_st_03"),
        "85159cea8663ed720be88ca0ee008a5830b0829a" => Some("c0.0.22a_05"),
        "83b6483feb88136b6b4662b553d8f80f5f88efa5" => Some("c0.0.21a"),
        "c2f8fddde4691d7c567c0c049ad4d03eb6b9e61c" => Some("c0.0.20a_01"),
        "e2b248f1013933af9f801729418409fb7198de1b" => Some("c0.0.19a_06-2"),
        "a78468abd491d6c661c000f60d6270a692ba4710" => Some("c0.0.18a_02"),
        "ca840460a6589552c9d1978ca121bf3e7c16a010" => Some("c0.0.17a"),
        "741eb3f84097fdcc0327230e018a0f8cd39addfb" => Some("c0.0.16a_02"),
        "936d575b1ab1a04a341ad43d76e441e88d2cd987" => Some("c0.0.13a"),
        "e8aa74a5bee547097375d44ffb2e407b2ea8ee4d" => Some("c0.0.14a_08"),
        "b9884f960f2b28a36b34db3447963f1ff4058aa4" => Some("c0.0.23a_01"),
        "7ba9e63aec8a15a99ecd47900c848cdce8a51a03" => Some("c0.0.13a_03"),
        "501ea8a6274faffe0144d3b24ed56797ce0765ff" => Some("c0.0.12a_03"),
        "3a799f179b6dcac5f3a46846d687ebbd95856984" => Some("c0.0.11a"),
        "6323bd14ed7f83852e17ebc8ec418e55c97ddfe4" => Some("rd-161348"),
        "b100be8097195b6c9112046dc6a80d326c8df839" => Some("rd-160052"),
        "12dace5a458617d3f90337a7ebde86c0593a6899" => Some("rd-132328"),
        "393e8d4b4d708587e2accd7c5221db65365e1075" => Some("rd-132211"),
        _ => None,
    }
}

/// 从 JAR 文件读取 Minecraft 版本号
/// 依次尝试：JAR 内 version.json → Minecraft.class 常量池 → MinecraftServer.class 常量池
pub fn from_jar(jar_path: &str) -> Option<String> {
    if !Path::new(jar_path).is_file() {
        return None;
    }

    let file = File::open(jar_path).ok()?;
    let mut jar = ZipArchive::new(file).ok()?;

    // 1. 尝试 JAR 内的 version.json（Minecraft 1.14+）
    if let Ok(mut entry) = jar.by_name("version.json") {
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_ok() {
            let content = String::from_utf8_lossy(&buf);
            if let Some(version) = from_version_json(&content) {
                return Some(version);
            }
        }
    }

    // 2. 尝试 Minecraft.class 常量池
    if let Ok(mut entry) = jar.by_name("net/minecraft/client/Minecraft.class") {
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_ok() {
            if let Some(version) = from_minecraft_class(&buf) {
                // 过滤 RC/Beta/Alpha 的内部标记
                // ⚠️ 源注释称"回退到下一个方法"，但实际代码为 return null（整函数退出），照代码移植
                if version == "RC1" || version == "RC2" {
                    return None;
                }
                if let Some(rest) = version.strip_prefix("Beta ") {
                    return Some(format!("b{rest}"));
                }
                if let Some(rest) = version.strip_prefix("Alpha v") {
                    return Some(format!("a{rest}"));
                }
                return Some(version);
            }
        }
    }

    // 3. 尝试 MinecraftServer.class 常量池
    if let Ok(mut entry) = jar.by_name("net/minecraft/server/MinecraftServer.class") {
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_ok() {
            if let Some(version) = from_minecraft_server_class(&buf) {
                return Some(version);
            }
        }
    }

    // 4. 尝试已知版本号映射
    let bytes = std::fs::read(jar_path).ok()?;
    let mut hasher = sha1::Sha1::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let hash = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    if let Some(known) = known_version(&hash) {
        return Some(known.to_string());
    }

    None
}

/// 从 JAR 内的 version.json 读取 id 字段
/// （源签名接收 jar + entry 并自行打开流；此处由调用方读入文本后传入，逻辑等价）
fn from_version_json(content: &str) -> Option<String> {
    let obj: serde_json::Map<String, Value> = serde_json::from_str(content).ok()?;
    let id = obj.get("id")?.as_str()?;
    if id.is_empty() {
        return None;
    }

    let slash_index = id.find(" / ");
    match slash_index {
        Some(idx) => Some(id[..idx].to_string()),
        None => Some(id.to_string()),
    }
}

/// 从 Minecraft.class 常量池中查找 "Minecraft Minecraft X.Y.Z" 字符串
fn from_minecraft_class(bytes: &[u8]) -> Option<String> {
    let utf8_strings = read_constant_pool_utf8_strings(bytes).ok()?;

    for s in utf8_strings {
        if let Some(rest) = s.strip_prefix("Minecraft Minecraft ") {
            return Some(rest.to_string());
        }
    }

    None
}

/// 从 MinecraftServer.class 常量池中查找版本号
/// 策略：找到 "Can't keep up!" 附近最近的版本号格式字符串
fn from_minecraft_server_class(bytes: &[u8]) -> Option<String> {
    let utf8_strings = read_constant_pool_utf8_strings(bytes).ok()?;

    let can_keep_up_idx = utf8_strings
        .iter()
        .position(|s| s.starts_with("Can't keep up!"))?;

    let version_pattern = Regex::new(r"^.*\d.*$").ok()?;
    for s in utf8_strings.iter().take(can_keep_up_idx).rev() {
        if version_pattern.is_match(s) {
            return Some(s.clone());
        }
    }

    None
}

/// 解析 Java .class 文件常量池，提取所有 CONSTANT_Utf8 字符串
fn read_constant_pool_utf8_strings(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut strings = Vec::new();
    let mut pos = 0usize;

    // Magic number (0xCAFEBABE)
    let magic = read_u32_be(bytes, &mut pos)?;
    if magic != 0xCAFEBABE {
        return Err("Not a valid Java class file".into());
    }

    // minor_version, major_version
    read_u16_be(bytes, &mut pos)?;
    read_u16_be(bytes, &mut pos)?;

    // constant_pool_count
    let pool_count = read_u16_be(bytes, &mut pos)?;

    let mut i: u16 = 1;
    while i < pool_count {
        let tag = read_u8(bytes, &mut pos)?;

        match tag {
            CONSTANT_UTF8 => {
                let length = read_u16_be(bytes, &mut pos)? as usize;
                let value = bytes
                    .get(pos..pos + length)
                    .ok_or_else(|| "unexpected end of class file".to_string())?;
                pos += length;
                // 源 Encoding.UTF8.GetString：非法字节序列以 U+FFFD 替换（from_utf8_lossy 同语义）
                strings.push(String::from_utf8_lossy(value).into_owned());
            }

            CONSTANT_INTEGER | CONSTANT_FLOAT => {
                skip(bytes, &mut pos, 4)?;
            }

            CONSTANT_LONG | CONSTANT_DOUBLE => {
                skip(bytes, &mut pos, 8)?;
                i += 1; // Long 和 Double 占用两个常量池索引
            }

            CONSTANT_CLASS | CONSTANT_STRING | CONSTANT_METHODTYPE | CONSTANT_MODULE
            | CONSTANT_PACKAGE => {
                skip(bytes, &mut pos, 2)?;
            }

            CONSTANT_FIELDREF
            | CONSTANT_METHODREF
            | CONSTANT_INTERFACEMETHODREF
            | CONSTANT_NAMEANDTYPE
            | CONSTANT_DYNAMIC
            | CONSTANT_INVOKEDYNAMIC => {
                skip(bytes, &mut pos, 4)?;
            }

            CONSTANT_METHODHANDLE => {
                skip(bytes, &mut pos, 3)?;
            }

            _ => {
                return Err(format!("Unknown constant pool tag: {tag}"));
            }
        }

        i += 1;
    }

    Ok(strings)
}

fn read_u8(bytes: &[u8], pos: &mut usize) -> Result<u8, String> {
    let b = *bytes
        .get(*pos)
        .ok_or_else(|| "unexpected end of class file".to_string())?;
    *pos += 1;
    Ok(b)
}

fn skip(bytes: &[u8], pos: &mut usize, n: usize) -> Result<(), String> {
    let p = *pos;
    if bytes.len() < p + n {
        return Err("unexpected end of class file".to_string());
    }
    *pos = p + n;
    Ok(())
}

fn read_u16_be(bytes: &[u8], pos: &mut usize) -> Result<u16, String> {
    let p = *pos;
    let slice = bytes
        .get(p..p + 2)
        .ok_or_else(|| "unexpected end of class file".to_string())?;
    *pos = p + 2;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32_be(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    let p = *pos;
    let slice = bytes
        .get(p..p + 4)
        .ok_or_else(|| "unexpected end of class file".to_string())?;
    *pos = p + 4;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
