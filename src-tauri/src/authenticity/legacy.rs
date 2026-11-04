//! 旧版清单（c2pa-rs 0.90 及更早）的只读识别。
//!
//! 0.90 把 `c2pa.soft-binding` 的 `value` 写成 CBOR 文本串（tstr），而 C2PA 规范
//! 要求字节串（bstr）。0.91 起按规范严格解析，读到 tstr 会把整条清单判为
//! `claim.malformed`。也就是说，升级 `c2pa` 之后，以前版本发布的文件并没有
//! 损坏，却会从「可验证」直接变成「读不出来」。
//!
//! 这里只补上**识别**所需的最小读取：从 JPEG 的 APP11/JUMBF 里取出
//! `com.lilith.artworks.claim`、`c2pa.soft-binding` 与 claim 的 `dc:title`，
//! 其余一律不碰：
//!
//! - 不验签。没有证书链、时间戳与哈希校验，结果必须由调用方标记为「未验证」；
//! - 不解析图片、不解码其它断言、不读取签名盒；
//! - 长度、层级、条目数都有上限，任何结构异常都返回 `None`，绝不 panic；
//! - 找不到本应用的 claim 断言就返回 `None`，不会把第三方清单一律当成旧格式。
//!
//! 这条路径的输入是**用户提供的任意文件**，和 `c2pa::read_manifest` 一样属于不受
//! 信任的字节；因此解析器只做「读到就返回、读不懂就放弃」，不做任何恢复性猜测。

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use super::{
    c2pa::{record_id_from_value, watermark_id_from_value, ASSERTION_LABEL},
    trustmark::identifier_from_soft_binding,
};

/// 单个 JPEG 里允许收集的 JUMBF 字节上限。实际清单通常几十 KB，签名断言约 12 KB。
const MAX_STORE_BYTES: usize = 8 * 1024 * 1024;
/// 扫描的 JPEG 段数量上限；清单必须出现在 SOS 之前。
const MAX_SEGMENTS: usize = 512;
/// 一次解析允许访问的 JUMBF 盒数量上限，防止超长盒链拖住识别。
const MAX_BOXES: usize = 256;
/// CBOR 嵌套深度上限。
const MAX_CBOR_DEPTH: usize = 16;
/// 单次 CBOR 解码的条目总数上限。
const MAX_CBOR_ITEMS: usize = 4096;
/// 单个文本值的字节上限：本应用的声明都是短字符串。
const MAX_TEXT_BYTES: usize = 4096;

/// 旧版清单中与识别有关的最小字段集合。
#[derive(Debug, Default)]
pub(crate) struct LegacyManifest {
    pub(crate) record_id: Option<String>,
    pub(crate) watermark_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) creator: Option<String>,
    pub(crate) rights_statement: Option<String>,
    pub(crate) authentication_content: Option<String>,
    /// 软绑定值是 0.90 的 CBOR 文本串。只有这一情形才是严格读取器拒绝的原因；
    /// 调用方据此区分「旧格式」与「当前格式但验证失败」。
    pub(crate) legacy_soft_binding: bool,
}

/// 读取旧版清单。只支持 JPEG（本应用只输出 JPEG），其它格式返回 `None`。
pub(crate) fn read(path: &Path) -> Option<LegacyManifest> {
    let store = jumbf_store(path)?;
    let mut budget = MAX_BOXES;
    let store_box = child_boxes(&store, &mut budget)
        .into_iter()
        .find(|item| item.superbox && item.label.as_deref() == Some("c2pa"))?;
    let mut manifest = None;
    for item in child_boxes(store_box.body, &mut budget) {
        if !item.superbox {
            continue;
        }
        let Some(label) = item.label.as_deref() else {
            continue;
        };
        if !label.starts_with("urn:c2pa:") {
            continue;
        }
        // 清单仓库可以保存多份清单，最后一份是活动清单，与 `Reader::active_manifest` 一致。
        if let Some(parsed) = read_manifest_box(item.body, &mut budget) {
            manifest = Some(parsed);
        }
    }
    manifest
}

/// 从 JPEG 头部收集 JUMBF 字节。遇到 SOS/EOI 或结构错误即停止。
fn jumbf_store(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut signature = [0_u8; 2];
    file.read_exact(&mut signature).ok()?;
    if signature != [0xFF, 0xD8] {
        return None;
    }

    let mut store = Vec::new();
    for _ in 0..MAX_SEGMENTS {
        let marker = next_marker(&mut file)?;
        match marker {
            // 图像数据开始或文件结束：清单必须在此之前。
            0xD9 | 0xDA => break,
            // 无长度的独立标记。
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let mut length = [0_u8; 2];
        file.read_exact(&mut length).ok()?;
        let length = u16::from_be_bytes(length) as usize;
        if length < 2 {
            return None;
        }
        let payload_length = length - 2;
        if marker == 0xEB {
            let mut payload = vec![0_u8; payload_length];
            file.read_exact(&mut payload).ok()?;
            // APP11 中的 JUMBF 以 "JP" 开头，随后是 2 字节盒实例号与 4 字节包序号。
            if payload.len() >= 8 && payload.starts_with(b"JP") {
                let body = &payload[8..];
                if store.len().checked_add(body.len())? > MAX_STORE_BYTES {
                    return None;
                }
                store.extend_from_slice(body);
            }
        } else {
            file.seek(SeekFrom::Current(payload_length as i64)).ok()?;
        }
    }

    (!store.is_empty()).then_some(store)
}

/// 读取下一个 JPEG 标记，跳过 `0xFF` 填充字节。
fn next_marker(file: &mut File) -> Option<u8> {
    loop {
        let mut prefix = [0_u8; 1];
        file.read_exact(&mut prefix).ok()?;
        if prefix[0] != 0xFF {
            return None;
        }
        let mut code = [0_u8; 1];
        file.read_exact(&mut code).ok()?;
        if code[0] == 0xFF {
            continue;
        }
        return Some(code[0]);
    }
}

struct JumbfBox<'a> {
    kind: [u8; 4],
    label: Option<String>,
    body: &'a [u8],
    superbox: bool,
}

/// 解析一层 JUMBF 盒。长度越界、描述盒损坏或预算用尽都只停止解析，不报错。
fn child_boxes<'a>(buffer: &'a [u8], budget: &mut usize) -> Vec<JumbfBox<'a>> {
    let mut boxes = Vec::new();
    let mut offset = 0_usize;
    while offset.saturating_add(8) <= buffer.len() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Some((length, header)) = box_extent(&buffer[offset..]) else {
            break;
        };
        let Some(end) = offset.checked_add(length) else {
            break;
        };
        if end > buffer.len() {
            break;
        }
        let mut kind = [0_u8; 4];
        kind.copy_from_slice(&buffer[offset + 4..offset + 8]);
        let body = &buffer[offset + header..end];
        if &kind == b"jumb" {
            if let Some((label, content)) = description(body) {
                boxes.push(JumbfBox {
                    kind,
                    label,
                    body: content,
                    superbox: true,
                });
            }
        } else {
            boxes.push(JumbfBox {
                kind,
                label: None,
                body,
                superbox: false,
            });
        }
        offset = end;
    }
    boxes
}

fn box_extent(buffer: &[u8]) -> Option<(usize, usize)> {
    let length = u32::from_be_bytes(buffer.get(..4)?.try_into().ok()?) as usize;
    match length {
        0 => Some((buffer.len(), 8)),
        1 => {
            let extended = u64::from_be_bytes(buffer.get(8..16)?.try_into().ok()?);
            let length = usize::try_from(extended).ok()?;
            (length >= 16).then_some((length, 16))
        }
        length => (length >= 8).then_some((length, 8)),
    }
}

/// 读取 superbox 的描述盒，返回标签与后续内容。
fn description(body: &[u8]) -> Option<(Option<String>, &[u8])> {
    let length = u32::from_be_bytes(body.get(..4)?.try_into().ok()?) as usize;
    if body.get(4..8)? != b"jumd" || length < 8 || length > body.len() {
        return None;
    }
    let description = &body[8..length];
    let toggles = *description.get(16)?;
    let label = if toggles & 0x01 != 0 {
        let text = description.get(17..)?;
        let end = text.iter().position(|byte| *byte == 0)?;
        Some(std::str::from_utf8(&text[..end]).ok()?.to_owned())
    } else {
        None
    };
    Some((label, &body[length..]))
}

fn read_manifest_box(body: &[u8], budget: &mut usize) -> Option<LegacyManifest> {
    let mut claim = None;
    let mut declared = None;
    let mut soft_binding = None;
    for item in child_boxes(body, budget) {
        let Some(label) = item.label.as_deref() else {
            continue;
        };
        if !item.superbox {
            continue;
        }
        match label {
            // c2pa 只用 `.vN` 后缀标记 v2 及以上；v1 的标签就是 `c2pa.claim`。
            "c2pa.claim" | "c2pa.claim.v2" => claim = assertion_value(item.body),
            // 自定义声明与软绑定都在断言仓库里，是 manifest 的孙辈。
            "c2pa.assertions" => {
                for assertion in child_boxes(item.body, budget) {
                    if !assertion.superbox {
                        continue;
                    }
                    match assertion.label.as_deref() {
                        Some(ASSERTION_LABEL) => declared = assertion_value(assertion.body),
                        Some("c2pa.soft-binding") => {
                            soft_binding = assertion_value(assertion.body);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    // 只处理本应用写出的清单；第三方清单交给严格读取器。
    let declared = declared?;
    let record_id = declared
        .get("recordId")
        .and_then(CborValue::text)
        .and_then(record_id_from_value);
    let (watermark_id, legacy_soft_binding) = match soft_binding.as_ref() {
        Some(binding) => match soft_binding_watermark(binding) {
            SoftBinding::Legacy(identifier) => (identifier, true),
            // 字节串是规范写法（0.91 起），说明这不是需要兼容的旧清单。
            SoftBinding::NewFormat => return None,
            SoftBinding::Missing => (
                declared
                    .get("watermarkId")
                    .and_then(CborValue::text)
                    .and_then(watermark_id_from_value),
                false,
            ),
        },
        None => (
            declared
                .get("watermarkId")
                .and_then(CborValue::text)
                .and_then(watermark_id_from_value),
            false,
        ),
    };

    // 还没有 recordId 的历史清单可以按 TrustMark ID 识别；两者都没有就没有识别价值。
    if record_id.is_none() && watermark_id.is_none() {
        return None;
    }

    Some(LegacyManifest {
        record_id,
        watermark_id,
        title: claim
            .as_ref()
            .and_then(|value| value.get("dc:title"))
            .and_then(CborValue::text)
            .map(str::to_owned),
        creator: declared
            .get("creator")
            .and_then(CborValue::text)
            .map(str::to_owned),
        rights_statement: declared
            .get("rightsStatement")
            .and_then(CborValue::text)
            .map(str::to_owned),
        authentication_content: declared
            .get("authenticationContent")
            .and_then(CborValue::text)
            .map(str::to_owned),
        legacy_soft_binding,
    })
}

enum SoftBinding {
    Legacy(Option<String>),
    NewFormat,
    Missing,
}

/// 旧清单的软绑定值必须是文本串 `1*<40 位二进制>`；字节串意味着规范写法。
fn soft_binding_watermark(binding: &CborValue) -> SoftBinding {
    let Some(block) = binding
        .get("blocks")
        .and_then(CborValue::array)
        .and_then(|blocks| blocks.first())
    else {
        return SoftBinding::Missing;
    };
    match block.get("value") {
        Some(CborValue::Text(value)) => SoftBinding::Legacy(identifier_from_soft_binding(value)),
        Some(CborValue::Bytes) => SoftBinding::NewFormat,
        _ => SoftBinding::Missing,
    }
}

/// 取出断言盒里的第一个内容盒并解码。旧版 `c2pa` 写 CBOR，这里只认 CBOR。
fn assertion_value(body: &[u8]) -> Option<CborValue> {
    let mut budget = MAX_BOXES;
    for item in child_boxes(body, &mut budget) {
        if item.superbox {
            continue;
        }
        if &item.kind == b"cbor" {
            return decode_cbor(item.body);
        }
    }
    None
}

#[derive(Debug, PartialEq)]
enum CborValue {
    Null,
    Bool(bool),
    Int(i64),
    /// 字节串只保留「类型」：旧格式兼容只关心它是不是字节串，不需要内容。
    Bytes,
    Text(String),
    Array(Vec<CborValue>),
    Map(Vec<(String, CborValue)>),
}

impl CborValue {
    fn get(&self, key: &str) -> Option<&CborValue> {
        match self {
            CborValue::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            CborValue::Text(value) => Some(value),
            _ => None,
        }
    }

    fn array(&self) -> Option<&[CborValue]> {
        match self {
            CborValue::Array(values) => Some(values),
            _ => None,
        }
    }
}

/// 最小 CBOR 解码：只接受长度确定的项，不支持不定长与简单值扩展。
fn decode_cbor(bytes: &[u8]) -> Option<CborValue> {
    let mut budget = MAX_CBOR_ITEMS;
    let (value, consumed) = decode_item(bytes, 0, 0, &mut budget)?;
    // 内容盒可能带零填充；非零尾随字节说明结构不对。
    if bytes.get(consumed..)?.iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(value)
}

fn decode_item(
    buffer: &[u8],
    offset: usize,
    depth: usize,
    budget: &mut usize,
) -> Option<(CborValue, usize)> {
    if depth > MAX_CBOR_DEPTH || *budget == 0 {
        return None;
    }
    *budget -= 1;

    let initial = *buffer.get(offset)?;
    let major = initial >> 5;
    let additional = initial & 0x1F;
    let (argument, cursor) = match additional {
        0..=23 => (additional as u64, offset + 1),
        24 => (*buffer.get(offset + 1)? as u64, offset + 2),
        25 => (
            u16::from_be_bytes(buffer.get(offset + 1..offset + 3)?.try_into().ok()?) as u64,
            offset + 3,
        ),
        26 => (
            u32::from_be_bytes(buffer.get(offset + 1..offset + 5)?.try_into().ok()?) as u64,
            offset + 5,
        ),
        27 => (
            u64::from_be_bytes(buffer.get(offset + 1..offset + 9)?.try_into().ok()?),
            offset + 9,
        ),
        // 不定长（31）与保留值不做兼容。
        _ => return None,
    };

    match major {
        0 => Some((CborValue::Int(i64::try_from(argument).ok()?), cursor)),
        1 => {
            let value = -1_i128 - i128::from(argument);
            Some((CborValue::Int(i64::try_from(value).ok()?), cursor))
        }
        2 => {
            let length = usize::try_from(argument).ok()?;
            let end = cursor.checked_add(length)?;
            buffer.get(cursor..end)?;
            Some((CborValue::Bytes, end))
        }
        3 => {
            let length = usize::try_from(argument).ok()?;
            if length > MAX_TEXT_BYTES {
                return None;
            }
            let end = cursor.checked_add(length)?;
            let text = std::str::from_utf8(buffer.get(cursor..end)?).ok()?;
            Some((CborValue::Text(text.to_owned()), end))
        }
        4 => {
            let length = usize::try_from(argument).ok()?;
            if length > *budget {
                return None;
            }
            let mut values = Vec::with_capacity(length.min(16));
            let mut cursor = cursor;
            for _ in 0..length {
                let (value, next) = decode_item(buffer, cursor, depth + 1, budget)?;
                values.push(value);
                cursor = next;
            }
            Some((CborValue::Array(values), cursor))
        }
        5 => {
            let length = usize::try_from(argument).ok()?;
            if length > *budget {
                return None;
            }
            let mut entries = Vec::with_capacity(length.min(8));
            let mut cursor = cursor;
            for _ in 0..length {
                let (key, next) = decode_item(buffer, cursor, depth + 1, budget)?;
                let CborValue::Text(key) = key else {
                    return None;
                };
                let (value, next) = decode_item(buffer, next, depth + 1, budget)?;
                entries.push((key, value));
                cursor = next;
            }
            Some((CborValue::Map(entries), cursor))
        }
        // 标签对识别无意义，透明跳过（半精度/大整数标签也在内）。
        6 => decode_item(buffer, cursor, depth + 1, budget),
        7 => match additional {
            20 => Some((CborValue::Bool(false), cursor)),
            21 => Some((CborValue::Bool(true), cursor)),
            22 => Some((CborValue::Null, cursor)),
            26 | 27 => Some((CborValue::Null, cursor)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::authenticity::test_support::{
        fixture_directory, FIXTURE_RECORD_ID, FIXTURE_WATERMARK_ID,
    };
    use crate::fuzz_support::random_bytes;

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.jpg");
        fs::write(&path, bytes).unwrap();
        (directory, path)
    }

    #[test]
    fn reads_manifests_signed_before_the_encoding_change() {
        let manifest = read(&fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();

        assert!(manifest.legacy_soft_binding);
        assert_eq!(manifest.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(manifest.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert_eq!(manifest.title.as_deref(), Some("Lilith C2PA fixture"));
        assert_eq!(manifest.creator.as_deref(), Some("Lilith Artworks tests"));
        assert_eq!(
            manifest.rights_statement.as_deref(),
            Some("Regression fixture only")
        );
        assert_eq!(
            manifest.authentication_content.as_deref(),
            Some("A tiny deterministic TrustMark/C2PA sample")
        );
    }

    #[test]
    fn v1_claim_label_is_recognized() {
        // c2pa 只给 v2 及以上加 `.vN` 后缀；v1 的标签是 `c2pa.claim`。
        // 等长替换标签并补 NUL，验证标题仍从 claim 里读出。
        let mut bytes = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        let marker = b"c2pa.claim.v2";
        let offset = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        bytes[offset..offset + marker.len()].copy_from_slice(b"c2pa.claim\0\0\0");

        let (_directory, path) = write_temp(&bytes);
        let manifest = read(&path).unwrap();
        assert_eq!(manifest.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(manifest.title.as_deref(), Some("Lilith C2PA fixture"));
    }

    #[test]
    fn unsigned_pictures_have_no_legacy_manifest() {
        assert!(read(&fixture_directory().join("source.jpg")).is_none());
    }

    /// 当前依赖签出的软绑定是规范要求的字节串，旧格式读取器必须拒绝它。
    /// 若某次升级又写回文本串，这条会先于发布流程报警。
    #[test]
    fn current_fixture_uses_the_byte_string_encoding() {
        assert!(read(&fixture_directory().join("valid-trustmark.jpg")).is_none());
    }

    #[test]
    fn a_manifest_without_the_application_claim_is_not_legacy() {
        let mut bytes = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        // 把自定义断言标签改成同样长度的其它标签：仍是合法清单，但不再属于本应用。
        let label = ASSERTION_LABEL.as_bytes();
        let replacement = b"com.lilith.artworks.fake!";
        assert_eq!(label.len(), replacement.len());
        let mut replaced = 0;
        for index in 0..bytes.len().saturating_sub(label.len()) {
            if &bytes[index..index + label.len()] == label {
                bytes[index..index + label.len()].copy_from_slice(replacement);
                replaced += 1;
            }
        }
        assert!(replaced > 0, "夹具里应包含自定义断言标签");

        let (_directory, path) = write_temp(&bytes);
        assert!(read(&path).is_none());
    }

    #[test]
    fn rewritten_soft_binding_watermark_is_still_read() {
        // 软绑定值是文本 `1*<id>`；替换成另一个 40 位 ID 后应读到新 ID。
        let identifier = "0101010101010101010101010101010101010101";
        let mut bytes = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        let marker = format!("1*{FIXTURE_WATERMARK_ID}");
        let offset = bytes
            .windows(marker.len())
            .position(|window| window == marker.as_bytes())
            .unwrap();
        bytes[offset + 2..offset + 2 + identifier.len()].copy_from_slice(identifier.as_bytes());

        let (_directory, path) = write_temp(&bytes);
        let manifest = read(&path).unwrap();
        assert_eq!(manifest.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(manifest.watermark_id.as_deref(), Some(identifier));
    }

    #[test]
    fn manifests_without_a_record_id_still_identify_by_watermark() {
        // 更早的清单可能没有 recordId；键名等长替换后仍应能按 TrustMark ID 识别。
        let mut bytes = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        let offset = bytes
            .windows(b"recordId".len())
            .position(|window| window == b"recordId")
            .unwrap();
        bytes[offset..offset + b"recordId".len()].copy_from_slice(b"recordXd");

        let (_directory, path) = write_temp(&bytes);
        let manifest = read(&path).unwrap();
        assert!(manifest.record_id.is_none());
        assert!(manifest.legacy_soft_binding);
        assert_eq!(manifest.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
    }

    #[test]
    fn byte_string_soft_binding_is_recognized_as_the_new_format() {
        // {"blocks": [{"value": h'313233'}]}
        let cbor = [
            0xA1, 0x66, b'b', b'l', b'o', b'c', b'k', b's', 0x81, 0xA1, 0x65, b'v', b'a', b'l',
            b'u', b'e', 0x43, 0x31, 0x32, 0x33,
        ];
        let value = decode_cbor(&cbor).unwrap();
        assert!(matches!(
            soft_binding_watermark(&value),
            SoftBinding::NewFormat
        ));
    }

    #[test]
    fn cbor_bombs_are_rejected() {
        // 声称有 2^61 个元素的数组。
        assert!(decode_cbor(&[0x9B, 0x20, 0, 0, 0, 0, 0, 0, 0]).is_none());
        // 深度超过上限的嵌套数组。
        let nested = [0x81_u8; MAX_CBOR_DEPTH + 2];
        assert!(decode_cbor(&nested).is_none());
        // 文本长度超出缓冲区。
        assert!(decode_cbor(&[0x7B, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_none());
        // 非文本键的映射。
        assert!(decode_cbor(&[0xA1, 0x01, 0x01]).is_none());
        // 不定长数组。
        assert!(decode_cbor(&[0x9F, 0x01, 0xFF]).is_none());
    }

    #[test]
    fn malformed_boxes_are_rejected_without_panicking() {
        let signed = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mutated.jpg");

        for index in (0..signed.len()).step_by(97) {
            let mut bytes = signed.clone();
            bytes[index] ^= 0x5A;
            fs::write(&path, &bytes).unwrap();
            let outcome = std::panic::catch_unwind(|| read(&path));
            assert!(outcome.is_ok(), "index {index}: 解析畸形清单时发生 panic");
        }
    }

    #[test]
    fn random_bytes_never_panic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("random.jpg");
        // 前半段带 JPEG/JUMBF 魔数，后半段完全随机，两种都应干净地返回。
        for seed in 0..64_u64 {
            let mut bytes = if seed % 2 == 0 {
                b"\xFF\xD8\xFF\xEB".to_vec()
            } else {
                b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00".to_vec()
            };
            bytes.extend_from_slice(&random_bytes(seed, 1 + (seed as usize * 17) % 4096));
            fs::write(&path, &bytes).unwrap();
            let outcome = std::panic::catch_unwind(|| read(&path));
            assert!(outcome.is_ok(), "seed {seed}: 解析随机字节时发生 panic");
        }
    }
}
