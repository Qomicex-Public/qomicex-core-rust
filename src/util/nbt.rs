//! NBT 解析 / 序列化（B2，对应 NbtIO.cs，高风险）
//!
//! 对应源：Services/Options/NbtIO.cs（servers.dat 的 NBT 读写）。
//! 语义要点：
//! - 源 `NbtCompound : Dictionary<string, object>`（StringComparer.Ordinal）→ 本模块
//!   `NbtCompound`（HashMap<String, NbtValue>；Rust String 相等即字节序相同，等价 Ordinal）
//! - 读入的 Byte 标签统一转为 bool（源 `ReadByte() != 0`）；写出时 bool 写回 1/0
//! - List 仅支持元素类型为 Compound（源对其它元素类型抛异常）
//! - 所有多字节整数均为大端（源 ReadInt32BigEndian / ReadUInt16BigEndian 语义）
//! - 字符串长度前缀为 u16 大端；UTF-8 解码采用替换语义（源 Encoding.UTF8.GetString
//!   遇无效字节替换为 U+FFFD，对应 Rust from_utf8_lossy）
//! - 源 Read 不支持压缩流（无 GZipStream/zlib），故本模块不依赖 flate2

use indexmap::IndexMap;
use std::io::{Read, Write};

/// NBT 标签类型常量（标准 NBT 规格 0..=11 全覆盖；
/// 源 NbtTagType 只声明了实际使用的 End/Byte/String/List/Compound 五个）
pub const TAG_END: u8 = 0;
pub const TAG_BYTE: u8 = 1;
pub const TAG_SHORT: u8 = 2;
pub const TAG_INT: u8 = 3;
pub const TAG_LONG: u8 = 4;
pub const TAG_FLOAT: u8 = 5;
pub const TAG_DOUBLE: u8 = 6;
pub const TAG_BYTE_ARRAY: u8 = 7;
pub const TAG_STRING: u8 = 8;
pub const TAG_LIST: u8 = 9;
pub const TAG_COMPOUND: u8 = 10;
pub const TAG_INT_ARRAY: u8 = 11;

/// NBT 标签值（对应源 `NbtCompound` 字典中的 object 值）。
/// 读/写仅支持源实现的那组类型：Byte(→bool)、String、List(仅 Compound 元素)、Compound。
/// 源对其它运行时类型抛 InvalidOperationException；Rust 侧类型系统使该分支不可达，
/// 故未移植（详见 p13 翻译日志）。
#[derive(Debug, Clone, PartialEq)]
pub enum NbtValue {
    /// TAG_Byte（源按 `ReadByte() != 0` 读为 bool）
    Byte(bool),
    /// TAG_String
    String(String),
    /// TAG_List（源仅支持元素类型为 Compound 的列表）
    List(Vec<NbtCompound>),
    /// TAG_Compound
    Compound(NbtCompound),
}

/// NBT 复合标签（对应源 NbtCompound : Dictionary<string, object>，
/// 键使用 Ordinal 比较——Rust String 相等即字节序相同，语义一致）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NbtCompound {
    entries: IndexMap<String, NbtValue>,
}

impl NbtCompound {
    /// 新建空复合标签
    pub fn new() -> Self {
        Self::default()
    }

    /// 取标签值（对应源 TryGetValue）
    pub fn get(&self, name: &str) -> Option<&NbtValue> {
        self.entries.get(name)
    }

    /// 写入标签值（对应源索引器赋值）
    pub fn insert(&mut self, name: String, value: NbtValue) {
        self.entries.insert(name, value);
    }

    /// 迭代全部标签（对应源 foreach (var entry in compound)）
    pub fn iter(&self) -> impl Iterator<Item = (&String, &NbtValue)> {
        self.entries.iter()
    }
}

/// NBT 读写错误（对应源 InvalidDataException / EndOfStreamException /
/// InvalidOperationException 三种异常）
#[derive(Debug, thiserror::Error)]
pub enum NbtError {
    /// 源：Expected root compound tag, but found type {tagType}.
    #[error("Expected root compound tag, but found type {0}.")]
    ExpectedRootCompound(u8),
    /// 源：Unsupported NBT tag type {tagType} in servers.dat.
    #[error("Unsupported NBT tag type {0} in servers.dat.")]
    UnsupportedTagType(u8),
    /// 源：NBT list length cannot be negative.
    #[error("NBT list length cannot be negative.")]
    NegativeListLength,
    /// 源：Unsupported NBT list element type {elementType} in servers.dat.
    #[error("Unsupported NBT list element type {0} in servers.dat.")]
    UnsupportedListElementType(u8),
    /// 源：Unexpected end of stream while reading {valueName}.
    #[error("Unexpected end of stream while reading {0}.")]
    UnexpectedEndOfStream(String),
    /// 源：NBT string length exceeds UInt16.MaxValue.
    #[error("NBT string length exceeds UInt16.MaxValue.")]
    StringTooLong,
    /// 源：Server entry tag '{name}' is not a string.
    #[error("Server entry tag '{0}' is not a string.")]
    NotAString(String),
    /// 源：Server entry tag '{name}' is not a byte/boolean.
    #[error("Server entry tag '{0}' is not a byte/boolean.")]
    NotAByteOrBool(String),
}

/// 读取根复合标签（对应源 NbtIO.Read：读类型必须是 Compound，
/// 根名称读出后丢弃，再读负载）
pub fn read<R: Read>(stream: &mut R) -> Result<NbtCompound, NbtError> {
    read_root_compound(stream)
}

/// 写出根复合标签（对应源 NbtIO.Write：写 Compound 类型、空名称、负载）
pub fn write<W: Write>(stream: &mut W, root: &NbtCompound) -> Result<(), NbtError> {
    write_root_compound(stream, root)
}

/// 取可选字符串标签（对应源 GetOptionalString：
/// 缺失返回 null → Rust None；存在但非字符串抛异常）
pub fn get_optional_string(compound: &NbtCompound, name: &str) -> Result<Option<String>, NbtError> {
    let Some(value) = compound.get(name) else {
        return Ok(None);
    };
    match value {
        NbtValue::String(text) => Ok(Some(text.clone())),
        _ => Err(NbtError::NotAString(name.to_string())),
    }
}

/// 取可选布尔标签（对应源 GetOptionalBool：
/// 缺失返回 false；Byte(bool) 直接返回；其它抛异常。
/// 注：源还兼容字典中手塞的原始 byte（byte number => number != 0），
/// Rust 类型系统下 NbtValue::Byte 已定型为 bool，该分支不可达）
pub fn get_optional_bool(compound: &NbtCompound, name: &str) -> Result<bool, NbtError> {
    let Some(value) = compound.get(name) else {
        return Ok(false);
    };
    match value {
        NbtValue::Byte(boolean) => Ok(*boolean),
        _ => Err(NbtError::NotAByteOrBool(name.to_string())),
    }
}

/// 对应源 ReadRootCompound
fn read_root_compound<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let tag_type = read_u8(reader, "NBT tag type")?;
    if tag_type != TAG_COMPOUND {
        return Err(NbtError::ExpectedRootCompound(tag_type));
    }
    // 源 `_ = ReadString(reader);`：根名称读出后丢弃
    read_string(reader)?;
    read_compound_payload(reader)
}

/// 对应源 WriteRootCompound
fn write_root_compound<W: Write>(writer: &mut W, root: &NbtCompound) -> Result<(), NbtError> {
    writer
        .write_all(&[TAG_COMPOUND])
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT root compound".into()))?;
    write_string(writer, "")?;
    write_compound_payload(writer, root)
}

/// 对应源 ReadCompoundPayload：循环读类型，End 结束；否则读名称 + 负载
fn read_compound_payload<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let mut compound = NbtCompound::new();
    loop {
        let tag_type = read_u8(reader, "NBT tag type")?;
        if tag_type == TAG_END {
            return Ok(compound);
        }
        let name = read_string(reader)?;
        let value = read_tag_payload(reader, tag_type)?;
        compound.entries.insert(name, value);
    }
}

/// 对应源 ReadTagPayload 的 switch（仅 Byte/String/List/Compound；
/// 其余类型抛 InvalidDataException）
fn read_tag_payload<R: Read>(reader: &mut R, tag_type: u8) -> Result<NbtValue, NbtError> {
    match tag_type {
        TAG_BYTE => Ok(NbtValue::Byte(read_u8(reader, "NBT Byte")? != 0)),
        TAG_STRING => Ok(NbtValue::String(read_string(reader)?)),
        TAG_LIST => read_list_payload(reader),
        TAG_COMPOUND => Ok(NbtValue::Compound(read_compound_payload(reader)?)),
        _ => Err(NbtError::UnsupportedTagType(tag_type)),
    }
}

/// 对应源 ReadListPayload：元素类型 + 大端 i32 长度；
/// 长度负数抛异常；元素类型非 Compound 抛异常
fn read_list_payload<R: Read>(reader: &mut R) -> Result<NbtValue, NbtError> {
    let element_type = read_u8(reader, "NBT list element type")?;
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeListLength);
    }
    if element_type != TAG_COMPOUND {
        return Err(NbtError::UnsupportedListElementType(element_type));
    }
    let mut items = Vec::with_capacity(length as usize);
    for _ in 0..length {
        items.push(read_compound_payload(reader)?);
    }
    Ok(NbtValue::List(items))
}

/// 对应源 WriteCompoundPayload：逐条目写命名标签，最后写 End
fn write_compound_payload<W: Write>(
    writer: &mut W,
    compound: &NbtCompound,
) -> Result<(), NbtError> {
    for (name, value) in compound.iter() {
        write_named_tag(writer, name, value)?;
    }
    writer
        .write_all(&[TAG_END])
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT tag type".into()))
}

/// 对应源 WriteNamedTag 的 switch：bool → Byte；String → String；
/// List(Compound) → List；Compound → Compound。
/// 源默认分支（不支持的类型抛 InvalidOperationException）在 Rust 类型系统下不可达
fn write_named_tag<W: Write>(
    writer: &mut W,
    name: &str,
    value: &NbtValue,
) -> Result<(), NbtError> {
    match value {
        NbtValue::Byte(boolean) => {
            writer
                .write_all(&[TAG_BYTE])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT tag type".into()))?;
            write_string(writer, name)?;
            writer
                .write_all(&[if *boolean { 1 } else { 0 }])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT Byte".into()))
        }
        NbtValue::String(text) => {
            writer
                .write_all(&[TAG_STRING])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT tag type".into()))?;
            write_string(writer, name)?;
            write_string(writer, text)
        }
        NbtValue::List(compounds) => {
            writer
                .write_all(&[TAG_LIST])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT tag type".into()))?;
            write_string(writer, name)?;
            writer
                .write_all(&[TAG_COMPOUND])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT list element type".into()))?;
            write_i32_big_endian(writer, compounds.len() as i32)?;
            for compound in compounds {
                write_compound_payload(writer, compound)?;
            }
            Ok(())
        }
        NbtValue::Compound(compound) => {
            writer
                .write_all(&[TAG_COMPOUND])
                .map_err(|_| NbtError::UnexpectedEndOfStream("NBT tag type".into()))?;
            write_string(writer, name)?;
            write_compound_payload(writer, compound)
        }
    }
}

/// 对应源 ReadString：u16 大端长度 + 按字节读取；
/// 长度不足抛 EndOfStreamException（源 ReadBytes 后校验长度）；
/// UTF-8 解码用替换语义（源 Encoding.UTF8.GetString）
fn read_string<R: Read>(reader: &mut R) -> Result<String, NbtError> {
    let length = read_u16_big_endian(reader)?;
    let mut bytes = vec![0u8; length as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT string".into()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 对应源 WriteString：UTF-8 字节长度超过 UInt16.MaxValue 抛
/// InvalidOperationException（源：NBT string length exceeds UInt16.MaxValue.）
fn write_string<W: Write>(writer: &mut W, value: &str) -> Result<(), NbtError> {
    let bytes = value.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(NbtError::StringTooLong);
    }
    write_u16_big_endian(writer, bytes.len() as u16)?;
    writer
        .write_all(bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT string".into()))
}

/// 对应源 ReadInt32BigEndian（源用 ReadExactly + 字节反转，语义等价 read_exact + from_be_bytes）
fn read_i32_big_endian<R: Read>(reader: &mut R) -> Result<i32, NbtError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT Int32".into()))?;
    Ok(i32::from_be_bytes(bytes))
}

/// 对应源 ReadUInt16BigEndian
fn read_u16_big_endian<R: Read>(reader: &mut R) -> Result<u16, NbtError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT UInt16".into()))?;
    Ok(u16::from_be_bytes(bytes))
}

/// 对应源 WriteInt32BigEndian
fn write_i32_big_endian<W: Write>(writer: &mut W, value: i32) -> Result<(), NbtError> {
    writer
        .write_all(&value.to_be_bytes())
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT Int32".into()))
}

/// 对应源 WriteUInt16BigEndian
fn write_u16_big_endian<W: Write>(writer: &mut W, value: u16) -> Result<(), NbtError> {
    writer
        .write_all(&value.to_be_bytes())
        .map_err(|_| NbtError::UnexpectedEndOfStream("NBT UInt16".into()))
}

/// 对应源 BinaryReader.ReadByte（EOF 时源抛 EndOfStreamException）
fn read_u8<R: Read>(reader: &mut R, value_name: &str) -> Result<u8, NbtError> {
    let mut byte = [0u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|_| NbtError::UnexpectedEndOfStream(value_name.to_string()))?;
    Ok(byte[0])
}

