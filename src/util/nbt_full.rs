//! 全量 NBT 解析 / 序列化（通用：存档 level.dat 等）。
//!
//! 来源：`Saves.cs` 内嵌 NBT 解析器（Services/Expansion/Local/Saves.cs，518 行），
//! 由 `services/local/saves.rs` 原私有实现（B10）提升为本公共模块，供存档设置
//! （level.dat NBT 编辑）等场景复用，避免重复实现。
//!
//! 覆盖标签类型：Byte/Short/Int/Long/Float/Double/String/List/Compound/
//! IntArray/LongArray（ByteArray(7) 源声明但读路径 switch 无分支 → 保留报错语义）。
//! 读取：gzip 魔数探测（0x1F 0x8B → 解压，否则原样）。写出：`write_gzip` 恒 gzip
//! 压缩（level.dat 语义，Minecraft 客户端不支持未压缩 level.dat）。
//!
//! ⚠️ 与 `util/nbt.rs`（B2，NbtIO.cs 专用）的差异：本模块 `NbtCompound` 用
//! `Vec<(String, NbtValue)>` 保**插入序**（写回字节布局稳定，同 .NET Dictionary
//! 无删除操作时按插入序迭代），且支持 Long/Int/Float/Double/IntArray/LongArray
//! 等全类型——B2 仅 Byte(→bool)/String/List(Compound)/Compound 四种。

use std::io::{Read, Write};

/// NBT 标签类型常量（标准 NBT 规格 0..=11 全覆盖；同源私有常量类 NbtTagType）。
const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
/// 源声明但读路径不支持（switch 无该分支 → 落入 default 抛异常 → 保留）
#[allow(dead_code)] // 源声明但读路径不支持（同源）
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// NBT 标签值（对应源私有 struct NbtValue：`TagType` 字节 + `object Value`；
/// object 的运行时类型分派 → Rust 枚举变体，TagType 由 tag_type() 推导，杜绝不一致）
#[derive(Debug, Clone, PartialEq)]
pub enum NbtValue {
    Byte(u8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(NbtList),
    Compound(NbtCompound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtValue {
    /// 对应源 `NbtValue.TagType` 字段（写出时写回类型字节）
    fn tag_type(&self) -> u8 {
        match self {
            NbtValue::Byte(_) => TAG_BYTE,
            NbtValue::Short(_) => TAG_SHORT,
            NbtValue::Int(_) => TAG_INT,
            NbtValue::Long(_) => TAG_LONG,
            NbtValue::Float(_) => TAG_FLOAT,
            NbtValue::Double(_) => TAG_DOUBLE,
            NbtValue::String(_) => TAG_STRING,
            NbtValue::List(_) => TAG_LIST,
            NbtValue::Compound(_) => TAG_COMPOUND,
            NbtValue::IntArray(_) => TAG_INT_ARRAY,
            NbtValue::LongArray(_) => TAG_LONG_ARRAY,
        }
    }
}

/// 对应源私有 sealed class NbtList（`ElementType` + `Items`）
#[derive(Debug, Clone, PartialEq)]
pub struct NbtList {
    element_type: u8,
    items: Vec<NbtValue>,
}

/// 对应源私有 sealed class NbtCompound : Dictionary<string, NbtValue>。
/// 用 `Vec<(String, NbtValue)>` 保插入序——.NET Dictionary 无删除操作时按插入序
/// 迭代 → 写回 level.dat 的条目顺序与源一致；替换已有键保原位（同 .NET）。
/// 查找 O(n)，level.dat 条目规模下无性能影响。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NbtCompound {
    entries: Vec<(String, NbtValue)>,
}

impl NbtCompound {
    /// 新建空复合标签
    pub fn new() -> Self {
        Self::default()
    }

    /// 对应源 Dictionary.TryGetValue
    pub fn get(&self, name: &str) -> Option<&NbtValue> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// 可变借用（用于改写已有键）
    pub fn get_mut(&mut self, name: &str) -> Option<&mut NbtValue> {
        self.entries
            .iter_mut()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// 对应源索引器赋值 `compound[name] = value`（已存在 → 原位替换，序不变；
    /// 不存在 → 追加到末尾，同 .NET Dictionary 插入序）
    pub fn insert(&mut self, name: String, value: NbtValue) {
        if let Some(entry) = self.entries.iter_mut().find(|(key, _)| *key == name) {
            entry.1 = value;
        } else {
            self.entries.push((name, value));
        }
    }

    /// 对应源 `foreach (var entry in compound)`（插入序）
    pub fn iter(&self) -> impl Iterator<Item = &(String, NbtValue)> {
        self.entries.iter()
    }
}

/// NBT 解析/写入错误（对应源 InvalidDataException / EndOfStreamException /
/// InvalidOperationException / IOException；消息文本保留源 Exception.Message）
#[derive(Debug, thiserror::Error)]
pub enum NbtError {
    /// 源 ReadRootCompound：Expected root compound tag, but found type {tagType}.
    #[error("Expected root compound tag, but found type {0}.")]
    ExpectedRootCompound(u8),
    /// 源 ReadTagPayload default 分支：Unsupported NBT tag type {tagType}.
    #[error("Unsupported NBT tag type {0}.")]
    UnsupportedNbtTagType(u8),
    /// 源 ReadListPayload：NBT list length cannot be negative.
    #[error("NBT list length cannot be negative.")]
    NegativeListLength,
    /// 源 ReadIntArrayPayload：NBT int array length cannot be negative.
    #[error("NBT int array length cannot be negative.")]
    NegativeIntArrayLength,
    /// 源 ReadLongArrayPayload：NBT long array length cannot be negative.
    #[error("NBT long array length cannot be negative.")]
    NegativeLongArrayLength,
    /// 源 WriteString：NBT string length exceeds UInt16.MaxValue.
    #[error("NBT string length exceeds UInt16.MaxValue.")]
    StringTooLong,
    /// 源 WriteLevelName 的 Data 复合缺失：level.dat does not contain Data compound.
    #[error("level.dat does not contain Data compound.")]
    NoDataCompound,
    /// 源 ReadString 的 EndOfStreamException / 其余 read_exact 到流尾
    /// （BinaryReader 各 Read 的 EndOfStreamException）
    #[error("Unexpected end of stream while reading NBT string.")]
    UnexpectedEndOfStream,
    /// 源 IO 异常（BinaryWriter 写失败 IOException / File 读写失败）
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 读取 NBT：gzip 魔数探测（0x1F 0x8B，前 2 字节不足 2 字节视为非 gzip）→
/// gzip 解压读取，否则原样读取（源 stream.Read 后复位 Position 语义等价整块字节判定）。
pub fn read(bytes: &[u8]) -> Result<NbtCompound, NbtError> {
    let is_gzip = bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B;
    let mut reader: Box<dyn Read + '_> = if is_gzip {
        Box::new(flate2::read::GzDecoder::new(bytes))
    } else {
        Box::new(bytes)
    };
    read_root_compound(&mut reader)
}

/// 根复合标签 → gzip 压缩字节（源：GZipStream(CompressionMode.Compress)，始终压缩；
/// 压缩级别为实现细节，.NET Optimal vs flate2 默认级别输出均为合法 gzip）。
pub fn write_gzip(root: &NbtCompound) -> Result<Vec<u8>, NbtError> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    write_root_compound(&mut encoder, root)?;
    Ok(encoder.finish()?)
}

/// 对应源 ReadRootCompound：读类型必须是 Compound，根名称读出后丢弃，再读负载
fn read_root_compound<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let tag_type = read_u8(reader)?;
    if tag_type != TAG_COMPOUND {
        return Err(NbtError::ExpectedRootCompound(tag_type));
    }
    // 源 `_ = ReadString(reader);`：根名称读出后丢弃
    read_string(reader)?;
    read_compound_payload(reader)
}

/// 对应源 WriteRootCompound：写 Compound 类型、空名称、负载
fn write_root_compound<W: Write>(writer: &mut W, root: &NbtCompound) -> Result<(), NbtError> {
    writer.write_all(&[TAG_COMPOUND])?;
    write_string(writer, "")?;
    write_compound_payload(writer, root)
}

/// 对应源 ReadCompoundPayload：循环读类型，End 结束；否则读名称 + 负载
fn read_compound_payload<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let mut compound = NbtCompound::new();
    loop {
        let tag_type = read_u8(reader)?;
        if tag_type == TAG_END {
            return Ok(compound);
        }
        let name = read_string(reader)?;
        let value = read_tag_payload(reader, tag_type)?;
        compound.insert(name, value);
    }
}

/// 对应源 ReadTagPayload 的 switch（10 种类型；源 ByteArray(7) 等未支持类型 →
/// switch default 抛 InvalidDataException）
fn read_tag_payload<R: Read>(reader: &mut R, tag_type: u8) -> Result<NbtValue, NbtError> {
    match tag_type {
        TAG_BYTE => Ok(NbtValue::Byte(read_u8(reader)?)),
        TAG_SHORT => Ok(NbtValue::Short(read_i16_big_endian(reader)?)),
        TAG_INT => Ok(NbtValue::Int(read_i32_big_endian(reader)?)),
        TAG_LONG => Ok(NbtValue::Long(read_i64_big_endian(reader)?)),
        TAG_FLOAT => Ok(NbtValue::Float(read_f32_big_endian(reader)?)),
        TAG_DOUBLE => Ok(NbtValue::Double(read_f64_big_endian(reader)?)),
        TAG_STRING => Ok(NbtValue::String(read_string(reader)?)),
        TAG_LIST => Ok(NbtValue::List(read_list_payload(reader)?)),
        TAG_COMPOUND => Ok(NbtValue::Compound(read_compound_payload(reader)?)),
        TAG_INT_ARRAY => Ok(NbtValue::IntArray(read_int_array_payload(reader)?)),
        TAG_LONG_ARRAY => Ok(NbtValue::LongArray(read_long_array_payload(reader)?)),
        _ => Err(NbtError::UnsupportedNbtTagType(tag_type)),
    }
}

/// 对应源 ReadListPayload：元素类型 + 大端 i32 长度；长度负数抛异常；
/// 元素类型任意（源经 ReadTagPayload 递归分派，与 B2 NbtIO 的
/// 仅 Compound 元素约束不同——此处按源 Saves.cs 语义）
fn read_list_payload<R: Read>(reader: &mut R) -> Result<NbtList, NbtError> {
    let element_type = read_u8(reader)?;
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeListLength);
    }
    let mut items = Vec::with_capacity(length as usize);
    for _ in 0..length {
        items.push(read_tag_payload(reader, element_type)?);
    }
    Ok(NbtList {
        element_type,
        items,
    })
}

/// 对应源 ReadIntArrayPayload：大端 i32 长度（负数抛异常）+ 大端 i32 元素
fn read_int_array_payload<R: Read>(reader: &mut R) -> Result<Vec<i32>, NbtError> {
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeIntArrayLength);
    }
    let mut array = Vec::with_capacity(length as usize);
    for _ in 0..length {
        array.push(read_i32_big_endian(reader)?);
    }
    Ok(array)
}

/// 对应源 ReadLongArrayPayload：大端 i32 长度（负数抛异常）+ 大端 i64 元素
fn read_long_array_payload<R: Read>(reader: &mut R) -> Result<Vec<i64>, NbtError> {
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeLongArrayLength);
    }
    let mut array = Vec::with_capacity(length as usize);
    for _ in 0..length {
        array.push(read_i64_big_endian(reader)?);
    }
    Ok(array)
}

/// 对应源 WriteCompoundPayload：逐条目写命名标签，最后写 End
fn write_compound_payload<W: Write>(writer: &mut W, compound: &NbtCompound) -> Result<(), NbtError> {
    for (name, value) in compound.iter() {
        write_named_tag(writer, name, value)?;
    }
    writer.write_all(&[TAG_END])?;
    Ok(())
}

/// 对应源 WriteNamedTag：类型字节 + 名称 + 负载。
/// 源 default 分支（不支持的值类型抛 InvalidOperationException）在 Rust
/// 类型系统下不可达（枚举覆盖全类型），同 B2 util/nbt.rs 的处理
fn write_named_tag<W: Write>(writer: &mut W, name: &str, tag: &NbtValue) -> Result<(), NbtError> {
    writer.write_all(&[tag.tag_type()])?;
    write_string(writer, name)?;
    write_tag_value(writer, tag)
}

/// 对应源 WriteTagValue（List 元素负载，无名称）：
/// 按运行时类型分派写出（源 switch (tag.Value)）
fn write_tag_value<W: Write>(writer: &mut W, tag: &NbtValue) -> Result<(), NbtError> {
    match tag {
        NbtValue::Byte(value) => writer.write_all(&[*value])?,
        NbtValue::Short(value) => write_i16_big_endian(writer, *value)?,
        NbtValue::Int(value) => write_i32_big_endian(writer, *value)?,
        NbtValue::Long(value) => write_i64_big_endian(writer, *value)?,
        NbtValue::Float(value) => write_f32_big_endian(writer, *value)?,
        NbtValue::Double(value) => write_f64_big_endian(writer, *value)?,
        NbtValue::String(value) => write_string(writer, value)?,
        NbtValue::List(list) => {
            // 源：写 ElementType + 大端 i32 长度 + 逐元素负载
            writer.write_all(&[list.element_type])?;
            write_i32_big_endian(writer, list.items.len() as i32)?;
            for item in &list.items {
                write_tag_value(writer, item)?;
            }
        }
        NbtValue::Compound(compound) => write_compound_payload(writer, compound)?,
        NbtValue::IntArray(array) => {
            // 源：大端 i32 长度 + 逐元素
            write_i32_big_endian(writer, array.len() as i32)?;
            for value in array {
                write_i32_big_endian(writer, *value)?;
            }
        }
        NbtValue::LongArray(array) => {
            write_i32_big_endian(writer, array.len() as i32)?;
            for value in array {
                write_i64_big_endian(writer, *value)?;
            }
        }
    }
    Ok(())
}

/// 对应源 ReadString：u16 大端长度 + 按字节读取；
/// 长度不足 → EndOfStreamException（源 ReadBytes 后校验长度）；
/// UTF-8 解码用替换语义（源 Encoding.UTF8.GetString 遇无效字节替换为 U+FFFD）
fn read_string<R: Read>(reader: &mut R) -> Result<String, NbtError> {
    let length = read_u16_big_endian(reader)?;
    let mut bytes = vec![0u8; length as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
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
    writer.write_all(bytes)?;
    Ok(())
}

// ── 大端读写原语（源 Read*/Write*BigEndian 全组：ReadExactly + 字节反转 ↔ read_exact + from_be_bytes）──

/// 对应源 ReadInt32BigEndian
fn read_i32_big_endian<R: Read>(reader: &mut R) -> Result<i32, NbtError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i32::from_be_bytes(bytes))
}

/// 对应源 WriteInt32BigEndian
fn write_i32_big_endian<W: Write>(writer: &mut W, value: i32) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadInt16BigEndian
fn read_i16_big_endian<R: Read>(reader: &mut R) -> Result<i16, NbtError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i16::from_be_bytes(bytes))
}

/// 对应源 WriteInt16BigEndian
fn write_i16_big_endian<W: Write>(writer: &mut W, value: i16) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadInt64BigEndian
fn read_i64_big_endian<R: Read>(reader: &mut R) -> Result<i64, NbtError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i64::from_be_bytes(bytes))
}

/// 对应源 WriteInt64BigEndian
fn write_i64_big_endian<W: Write>(writer: &mut W, value: i64) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadFloatBigEndian
fn read_f32_big_endian<R: Read>(reader: &mut R) -> Result<f32, NbtError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(f32::from_be_bytes(bytes))
}

/// 对应源 WriteFloatBigEndian
fn write_f32_big_endian<W: Write>(writer: &mut W, value: f32) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadDoubleBigEndian
fn read_f64_big_endian<R: Read>(reader: &mut R) -> Result<f64, NbtError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(f64::from_be_bytes(bytes))
}

/// 对应源 WriteDoubleBigEndian
fn write_f64_big_endian<W: Write>(writer: &mut W, value: f64) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadUInt16BigEndian
fn read_u16_big_endian<R: Read>(reader: &mut R) -> Result<u16, NbtError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(u16::from_be_bytes(bytes))
}

/// 对应源 WriteUInt16BigEndian
fn write_u16_big_endian<W: Write>(writer: &mut W, value: u16) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 BinaryReader.ReadByte（流尾 → EndOfStreamException）
fn read_u8<R: Read>(reader: &mut R) -> Result<u8, NbtError> {
    let mut byte = [0u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(byte[0])
}
