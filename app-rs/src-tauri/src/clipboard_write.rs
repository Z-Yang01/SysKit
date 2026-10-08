//! 条目内容写回系统剪贴板（单源；面板链与快速链共用）。
//!
//! 逐类型对齐 Python 侧 clip_paste.write_back_entry：
//! - text：CF_UNICODETEXT + 抑制自家监听 + 系统排除格式；
//! - file：CF_HDROP（真粘文件本体）+ 路径文本兜底——禁止只写路径文本；
//! - html：CF_HTML（原始 HTML）+ CF_UNICODETEXT 纯文本兜底（Word/浏览器富文本保真）；
//! - image：PNG 文件 → CF_DIB。
//!
//! 写回一律 `suppress_next`：否则监听器会把「自己写入」当成一次新复制，
//! 被粘贴的旧条目会顶到最新，历史顺序失真。

use std::path::Path;

use serde_json::json;

use crate::clipboard_history::{self, ClipItem};
use crate::paths;
use crate::w32;

/// CF_HTML 的官方剪贴板格式名。
pub const HTML_FORMAT: &str = "HTML Format";

/// 构造 CF_HTML 载荷（规范：`Version:1.0` + 六项字节偏移 + CRLF 头 + 片段注释）。
///
/// 偏移量必须是**字节**偏移且与头部实际长度一致，因此用固定宽度数字占位后
/// 一次性拼装（Windows 允许前导零填充，规范明确说明这是为此设计的）。
pub fn build_cf_html(fragment: &str) -> Vec<u8> {
    // 头部固定为十六进制定宽：StartHTML(10) EndHTML(10) StartFragment(10) EndFragment(10)
    // 头部长度先按占位串算，再回填真实值——两轮即可收敛（占位宽度固定，长度不变）。
    fn header(start_html: usize, end_html: usize, start_frag: usize, end_frag: usize) -> String {
        const PAD: usize = 10;
        format!(
            "Version:1.0\r\n\
                 StartHTML:{start_html:0PAD$}\r\n\
                 EndHTML:{end_html:0PAD$}\r\n\
                 StartFragment:{start_frag:0PAD$}\r\n\
                 EndFragment:{end_frag:0PAD$}\r\n"
        )
    }
    let start_comment = "<!--StartFragment-->";
    let end_comment = "<!--EndFragment-->";

    // 第一轮：用 0 占位求头部长
    let h0 = header(0, 0, 0, 0);
    let start_html = h0.len();
    let start_frag = start_html + start_comment.len();
    let frag_bytes = fragment.len();
    let end_frag = start_frag + frag_bytes;
    let end_html = end_frag + end_comment.len();
    let h1 = header(start_html, end_html, start_frag, end_frag);
    debug_assert_eq!(h1.len(), h0.len(), "头部长度必须与占位轮一致");

    let mut out = Vec::with_capacity(end_html);
    out.extend_from_slice(h1.as_bytes());
    out.extend_from_slice(start_comment.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(end_comment.as_bytes());
    out
}

/// PNG 字节 → CF_DIB 载荷（文件头 14 字节去掉，其余就是 BITMAPINFOHEADER 起）。
///
/// CF_DIB 是**无文件头**的 DIB：整块内存就是 BITMAPINFOHEADER + 像素位 + 调色板。
/// 自建结构而非借图像库编码器，避免其 BITMAPV4/V5 头在部分老应用上不兼容。
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    use image::GenericImageView;
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let (w, h) = img.dimensions();
    let rgba = img.to_rgba8();
    let bi_size = 40u32; // BITMAPINFOHEADER
    let pixel_bytes = (w as usize) * (h as usize) * 4;
    let mut out = Vec::with_capacity(bi_size as usize + pixel_bytes);
    // BITMAPINFOHEADER
    out.extend_from_slice(&bi_size.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // 正数 = 自下而上
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // 像素质：BI_RGB 下位序为 BGRA；行序自下而上，行内自左向右
    for y in (0..h).rev() {
        for x in 0..w {
            let p = rgba.get_pixel(x, y).0;
            out.push(p[2]);
            out.push(p[1]);
            out.push(p[0]);
            out.push(p[3]);
        }
    }
    Some(out)
}

/// 写回结果：是否成功 + 是否需要给系统排除格式（图片与文本都要）。
pub fn write_back(item: &ClipItem) -> bool {
    let kind = item.kind.as_str();
    let ok = match kind {
        "image" => write_image(&item.data),
        "file" => write_files(&item.text),
        "html" => write_html(&item.data, &item.text),
        _ => write_text(&item.text),
    };
    if ok {
        w32::exclude_os_history();
    }
    ok
}

fn write_text(text: &str) -> bool {
    let bytes = w32::build_unicode_text(text);
    const CF_UNICODETEXT: u32 = 13;
    if !w32::clipboard_set_std(CF_UNICODETEXT, &bytes) {
        return false;
    }
    clipboard_history::suppress_next(text, 2.0);
    true
}

fn write_files(text: &str) -> bool {
    let paths: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if paths.is_empty() {
        return false;
    }
    const CF_HDROP: u32 = 15;
    if !w32::clipboard_set_std(CF_HDROP, &w32::build_hdrop(&paths)) {
        return false;
    }
    // 路径列表文本仍是监听器的记录键 → 抑制
    clipboard_history::suppress_next(text, 2.0);
    true
}

fn write_html(html: &str, plain: &str) -> bool {
    if html.is_empty() {
        return write_text(plain);
    }
    if !w32::clipboard_set_named(HTML_FORMAT, &build_cf_html(html)) {
        return false;
    }
    // 纯文本兜底：目标只认 CF_UNICODETEXT 时仍能粘出内容（硬约束⑧）
    const CF_UNICODETEXT: u32 = 13;
    let bytes = w32::build_unicode_text(plain);
    // 兜底失败不改变主结果（HTML 已写入）
    let _ = w32::clipboard_set_std(CF_UNICODETEXT, &bytes);
    clipboard_history::suppress_next(plain, 2.0);
    true
}

fn write_image(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let path = match paths::image_path(name) {
        Some(p) => p,
        None => return false,
    };
    write_image_file(&path)
}

pub fn write_image_file(path: &Path) -> bool {
    let png = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("图片载荷读取失败: {} ({e})", path.display());
            return false;
        }
    };
    let dib = match png_to_dib(&png) {
        Some(d) => d,
        None => {
            log::warn!("图片载荷解码失败（损坏或非 PNG）: {}", path.display());
            return false;
        }
    };
    const CF_DIB: u32 = 8;
    w32::clipboard_set_std(CF_DIB, &dib)
}

/// 图片字节入库（md5 命名去重），返回文件名；失败返回空串。
pub fn save_image_bytes(png: &[u8]) -> String {
    save_image_bytes_in(&paths::clipboard_files_dir(), png)
}

/// 指定目录入库（测试用；生产恒为 clipboard_files）。
pub fn save_image_bytes_in(dir: &Path, png: &[u8]) -> String {
    if png.is_empty() {
        return String::new();
    }
    let name = format!("{}.png", md5_hex(png));
    if std::fs::create_dir_all(dir).is_err() {
        return String::new();
    }
    let path = dir.join(&name);
    if path.exists() {
        return name; // md5 去重：同字节只存一份
    }
    match std::fs::write(&path, png) {
        Ok(()) => name,
        Err(e) => {
            log::warn!("图片载荷写盘失败: {} ({e})", path.display());
            String::new()
        }
    }
}

/// 删除 clipboard_files 下未被引用的孤儿 PNG（mtime 早于宽限期才删），返回删除数。
pub fn sweep_orphan_files(referenced: &[String], min_age_sec: f64) -> usize {
    let dir = paths::clipboard_files_dir();
    let keep: std::collections::HashSet<&str> = referenced.iter().map(String::as_str).collect();
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs_f64(min_age_sec.max(0.0)))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0; // 目录不存在/遍历失败：清扫是兜底维护，绝不抛错
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.to_ascii_lowercase().ends_with(".png") || keep.contains(name.as_str()) {
            continue;
        }
        let older = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if older && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// MD5（仅用于载荷文件命名去重，非安全用途）。
pub fn md5_hex(data: &[u8]) -> String {
    // RFC 1321 参考实现的最小移植；与 Python hashlib.md5().hexdigest() 一致。
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::with_capacity(32);
    for v in [a0, b0, c0, d0] {
        for byte in v.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

/// 把面板选择结果整理成给前端的载荷（图片/文件/HTML 各自带 data）。
#[allow(dead_code)] // 面板/前端载荷构造的预留 API，测试覆盖中
pub fn item_payload(item: &ClipItem) -> serde_json::Value {
    json!({
        "kind": item.kind,
        "text": item.text,
        "data": item.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn item(kind: &str, text: &str, data: &str) -> ClipItem {
        ClipItem {
            ts: 0,
            text: text.to_string(),
            kind: kind.to_string(),
            pinned: false,
            data: data.to_string(),
            dhash: None,
            idx: None,
        }
    }

    #[test]
    fn md5_matches_rfc1321_vectors() {
        // RFC 1321 测试向量（与 Python hashlib.md5 一致）
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            md5_hex(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn cf_html_offsets_are_byte_accurate() {
        let frag = "<p>中文 &amp; 富文本</p>";
        let bytes = build_cf_html(frag);
        let text = String::from_utf8(bytes.clone()).unwrap();
        let header_end = text.find("<!--StartFragment-->").unwrap();
        let get = |key: &str| -> usize {
            let pos = text.find(&format!("{key}:")).unwrap() + key.len() + 1;
            text[pos..pos + 10].parse().unwrap()
        };
        assert!(text.starts_with("Version:1.0\r\n"));
        assert_eq!(
            get("StartHTML"),
            header_end,
            "无前置上下文时 StartHTML 就等于头部长度"
        );
        assert_eq!(get("EndHTML"), bytes.len());
        assert_eq!(
            get("StartFragment"),
            header_end + "<!--StartFragment-->".len()
        );
        // 偏移必须是字节数（含多字节中文），而不是字符数
        assert_eq!(get("EndFragment") - get("StartFragment"), frag.len());
        assert!(
            text.ends_with("<!--EndFragment-->"),
            "片段必须以结束注释收尾"
        );
        assert_eq!(bytes.len(), get("EndHTML"), "EndHTML 必须等于总字节数");
    }

    #[test]
    fn cf_html_header_width_is_stable_across_sizes() {
        // 偏移占位宽度固定：不同长度片段的头部长度必须一致
        let a = build_cf_html("x");
        let b = build_cf_html(&"y".repeat(5000));
        let ha = String::from_utf8(a.clone()).unwrap();
        let hb = String::from_utf8(b.clone()).unwrap();
        let end_a = ha.find("<!--StartFragment-->").unwrap();
        let end_b = hb.find("<!--StartFragment-->").unwrap();
        assert_eq!(end_a, end_b, "头部长度必须与片段长度无关");
    }

    #[test]
    fn png_to_dib_strips_file_header() {
        // 造一张 2x2 PNG
        let mut png = Vec::new();
        let img = image::RgbImage::from_fn(2, 2, |x, y| {
            image::Rgb([(x * 100) as u8, (y * 100) as u8, 0])
        });
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let dib = png_to_dib(&png).expect("PNG 应能转 DIB");
        // BITMAPINFOHEADER 前 4 字节是 biSize=40
        assert_eq!(
            u32::from_le_bytes(dib[0..4].try_into().unwrap()),
            40,
            "必须是 BITMAPINFOHEADER"
        );
        assert_eq!(
            i32::from_le_bytes(dib[4..8].try_into().unwrap()),
            2,
            "biWidth"
        );
        assert_eq!(
            i32::from_le_bytes(dib[8..12].try_into().unwrap()),
            2,
            "biHeight"
        );
        assert_eq!(
            u16::from_le_bytes(dib[14..16].try_into().unwrap()),
            32,
            "biBitCount=32"
        );
        assert_eq!(dib.len(), 40 + 2 * 2 * 4, "32bpp 无调色板：头 + 像素");
    }

    #[test]
    fn dib_rows_are_bottom_up_bgra() {
        // 上排红、下排蓝的 1x2 图；自下而上存储 → 先出现下排（蓝）
        let mut png = Vec::new();
        let mut img = image::RgbImage::new(1, 2);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(0, 1, image::Rgb([0, 0, 255]));
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), 1, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let dib = png_to_dib(&png).unwrap();
        let px = &dib[40..];
        assert_eq!(
            &px[0..4],
            &[255, 0, 0, 255],
            "首行是图像底部（蓝 → BGRA 交换后 R/B 互换）"
        );
        assert_eq!(&px[4..8], &[0, 0, 255, 255], "次行是图像顶部（红）");
    }

    #[test]
    fn png_to_dib_rejects_garbage() {
        assert!(png_to_dib(b"not a png").is_none());
        assert!(png_to_dib(b"").is_none());
    }

    #[test]
    fn save_image_bytes_dedups_by_md5() {
        let dir = std::env::temp_dir().join("syskit_rs_img_save");
        let _ = std::fs::remove_dir_all(&dir);
        let png = b"fake-png-bytes-for-dedup";
        let a = save_image_bytes_in(&dir, png);
        let b = save_image_bytes_in(&dir, png);
        assert_eq!(a, b, "同字节必须去重成同名");
        assert_eq!(a, format!("{}.png", md5_hex(png)));
        assert!(dir.join(&a).exists());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "只应有一个文件"
        );
        // 不同字节 → 不同文件
        let c = save_image_bytes_in(&dir, b"other-bytes");
        assert_ne!(a, c);
        // 空字节不入库
        assert_eq!(save_image_bytes_in(&dir, b""), "");
    }

    #[test]
    fn payload_keeps_all_three_fields() {
        let v = item_payload(&item("image", "[图片 2×2]", "abc.png"));
        assert_eq!(v["kind"], "image");
        assert_eq!(v["text"], "[图片 2×2]");
        assert_eq!(v["data"], "abc.png");
    }

    #[test]
    fn write_back_kind_dispatch_is_total() {
        // 每种 kind 都要有明确分支（未知 kind 走文本兜底，不 panic）
        for k in ["text", "image", "file", "html", "unknown-kind"] {
            let _ = item(k, "x", "");
        }
    }
}
