//! `CF_DIB` → RGBA/PNG：剪贴板位图捕获（补上 clipboard_watch 里"只登记占位条目"的缺口）。
//!
//! ## 为什么单独一个模块
//!
//! `CF_DIB` 是**无文件头**的 DIB：整块内存就是 `BITMAPINFOHEADER` + 可选掩码/调色板 +
//! 像素位。要正确还原必须处理一堆变量，而每一个都能让图像静默错位（不是崩溃，
//! 是**看起来正常的错图**——颜色反了、上下颠倒、行错位）：
//!
//! - `biHeight` **负数表示自上而下**，正数自下而上；搞反了图就倒过来；
//! - 行按 4 字节对齐（`stride = ((biBitCount * width + 31) / 32) * 4`），
//!   按 `width * bpp / 8` 取行会逐行错位；
//! - 32 位 `BI_RGB` 的字节序是 **BGRA**，且第 4 字节在旧格式里常是"未使用"（0）
//!   而不是 alpha——直接当 alpha 用会让整张图**全透明**；
//! - `BI_BITFIELDS`（3）与 `BI_ALPHABITFIELDS`（6）在头后紧跟 3~4 个 DWORD 掩码，
//!   调色板/像素**起点要相应后移**，否则把掩码当像素读；
//! - 索引色（1/4/8 位）要查调色板，调色板项是 **BGRX** 四字节。
//!
//! 本模块把上面前几条逐条断言，因为它们的错误形态都是"图能用但不对"。

use serde::Serialize;

/// 一个已解码的位图（RGBA8，行序自上而下）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` 字节，RGBA 顺序、自上而下
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// 像素数（用于生成 `[图片 W×H]` 占位文本）。
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// 是否值得记为图片条目：太小的（图标/光标残影）没有保留价值。
    ///
    /// 阈值按 Python 侧 `_ASYNC_ENCODE_PIXELS` 的下限取，避免把 16×16 图标
    /// 也塞进历史。
    pub fn is_worth_capturing(&self) -> bool {
        self.pixels() >= 16 * 16
    }
}

// BI_* 压缩方式
const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const BI_JPEG: u32 = 4;
const BI_PNG: u32 = 5;
const BI_ALPHABITFIELDS: u32 = 6;

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 行步长：按 4 字节对齐（`((bpp * width + 31) / 32) * 4`）。
pub fn row_stride(bit_count: u32, width: u32) -> u64 {
    let bits = u64::from(bit_count) * u64::from(width);
    bits.div_ceil(32) * 4
}

/// 从掩码取"该值对应多少位为 1"，并算出右移位数。
fn mask_shift(mask: u32) -> Option<(u32, u32)> {
    if mask == 0 {
        return None;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    Some((shift, bits))
}

/// 把掩码字段的值线性拉伸到 0~255。
///
/// 掩码来自**剪贴板里的不可信数据**：非连续或超宽掩码下 `value` 可远超
/// `2^bits - 1`，且 `bits` 可达 32——`1u32 << 32` 与 `value * 255` 在 debug
/// 构建直接溢出 panic（监听线程/调用线程当场死掉），release 静默错色。
/// 全程用 u64 计算并把结果夹紧到 0~255。
fn scale_field(value: u32, bits: u32) -> u8 {
    if bits == 0 || bits > 32 {
        return 0;
    }
    let max: u64 = if bits >= 32 {
        u32::MAX as u64
    } else {
        (1u64 << bits) - 1
    };
    let scaled = (u64::from(value) * 255 + max / 2) / max;
    scaled.min(255) as u8
}

/// 解析 `CF_DIB` 载荷 → RGBA 位图。
///
/// 不支持的形态返回 Err 并说明原因（**绝不返回尺寸对但内容错的图**）。
pub fn parse_dib(data: &[u8]) -> Result<Bitmap, String> {
    // BITMAPINFOHEADER 至少 40 字节；更小的（BITMAPCOREHEADER 12 字节）这里不支持
    let bi_size = u32_at(data, 0).ok_or("DIB 太短，读不出 biSize")?;
    if bi_size < 40 {
        return Err(format!(
            "不支持的 DIB 头大小 {bi_size}（BITMAPCOREHEADER 等旧格式未实现）"
        ));
    }
    let width_i = u32_at(data, 4).ok_or("读不出 biWidth")? as i32;
    let height_i = u32_at(data, 8).ok_or("读不出 biHeight")? as i32;
    let bit_count = u16_at(data, 14).ok_or("读不出 biBitCount")? as u32;
    let compression = u32_at(data, 16).ok_or("读不出 biCompression")?;
    let clr_used = u32_at(data, 32).unwrap_or(0);

    if width_i <= 0 || height_i == 0 {
        return Err(format!("尺寸非法: {width_i}×{height_i}"));
    }
    let width = width_i as u32;
    let top_down = height_i < 0;
    let height = height_i.unsigned_abs();

    // 尺寸上限保护：恶意/错乱的头部声明的尺寸可能极大，直接分配会 OOM
    const MAX_PIXELS: u64 = 100_000_000; // 1 亿像素（约 400MB RGBA）已远超剪贴板场景
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("图片过大: {width}×{height}"));
    }

    // 压缩方式：只支持无压缩与位域；RLE/JPEG/PNG 内嵌都不支持
    match compression {
        BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS => {}
        BI_RLE8 | BI_RLE4 => {
            return Err("暂不支持 RLE 压缩的位图".to_string());
        }
        BI_JPEG | BI_PNG => {
            // 这两种是"内嵌整张图"：biSizeImage 指向 PNG/JPEG 字节流，
            // 流的起点在**头之后**（biSize），不是写死的 40——V4/V5 头是 108/124
            // 字节，按 40 切会把头部余下字节混进图像流，解码必败
            return decode_embedded(data, bi_size as usize, compression);
        }
        other => return Err(format!("未知的压缩方式 {other}")),
    }

    // 掩码/调色板/像素的起点
    let mut pixel_off = bi_size as usize;
    let mut masks: Option<(u32, u32, u32, Option<u32>)> = None;
    if compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS {
        let r = u32_at(data, pixel_off).ok_or("读不出红色掩码")?;
        let g = u32_at(data, pixel_off + 4).ok_or("读不出绿色掩码")?;
        let b = u32_at(data, pixel_off + 8).ok_or("读不出蓝色掩码")?;
        pixel_off += 12;
        let a = if compression == BI_ALPHABITFIELDS {
            let a = u32_at(data, pixel_off).ok_or("读不出 alpha 掩码")?;
            pixel_off += 4;
            Some(a)
        } else {
            // 经典 BI_BITFIELDS 只有三个掩码（alpha 常按"全不透明"处理）
            None
        };
        masks = Some((r, g, b, a));
    }

    // 调色板：索引色或 clrUsed>0 时存在；每项 4 字节 BGRX
    let palette_entries = if bit_count <= 8 {
        if clr_used > 0 {
            clr_used as usize
        } else {
            1usize << bit_count
        }
    } else {
        clr_used as usize
    };
    let palette_bytes: usize = palette_entries * 4;
    let palette_slice = data
        .get(pixel_off..pixel_off + palette_bytes)
        .ok_or("调色板数据不足")?;
    let palette: Vec<[u8; 3]> = palette_slice
        .chunks_exact(4)
        .map(|c| [c[2], c[1], c[0]]) // BGRX → RGB
        .collect();
    pixel_off += palette_bytes;

    let stride = row_stride(bit_count, width);
    let total = stride
        .checked_mul(u64::from(height))
        .ok_or("像素数据长度溢出")? as usize;
    let pixels = data
        .get(pixel_off..pixel_off + total)
        .ok_or_else(|| format!("像素数据不足：需要 {total} 字节"))?;

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for row in 0..height as usize {
        // 行序：自下而上时源行号要反过来
        let src_row = if top_down {
            row
        } else {
            height as usize - 1 - row
        };
        let row_slice = &pixels[src_row * stride as usize..];
        let dst = &mut rgba[row * (width as usize) * 4..(row + 1) * (width as usize) * 4];
        match bit_count {
            32 => decode_32(row_slice, width, masks, dst),
            24 => decode_24(row_slice, width, dst),
            16 => decode_16(row_slice, width, masks, dst),
            8 => decode_indexed(row_slice, width, 8, &palette, dst),
            4 => decode_indexed(row_slice, width, 4, &palette, dst),
            1 => decode_indexed(row_slice, width, 1, &palette, dst),
            other => return Err(format!("不支持的位深 {other}")),
        }
    }
    Ok(Bitmap {
        width,
        height,
        rgba,
    })
}

/// 32 位：`BI_RGB` 下是 BGRA；有掩码时按掩码取。
///
/// **注意第 4 字节**：经典 `BI_RGB` 的 32 位图第 4 字节"未使用"常为 0，
/// 直接当 alpha 会让整张图全透明。所以无 alpha 掩码时一律按不透明处理。
fn decode_32(src: &[u8], width: u32, masks: Option<(u32, u32, u32, Option<u32>)>, dst: &mut [u8]) {
    for x in 0..width as usize {
        let o = x * 4;
        let (b, g, r) = (src[o], src[o + 1], src[o + 2]);
        let (rr, gg, bb, aa) = match masks {
            Some((mr, mg, mb, ma)) => {
                let v = u32::from_le_bytes([src[o], src[o + 1], src[o + 2], src[o + 3]]);
                let get = |m: u32, def: u8| -> u8 {
                    match mask_shift(m) {
                        Some((sh, bits)) => scale_field((v & m) >> sh, bits),
                        None => def,
                    }
                };
                let a = match ma {
                    Some(m) => get(m, 255),
                    None => 255,
                };
                (get(mr, r), get(mg, g), get(mb, b), a)
            }
            None => (r, g, b, 255),
        };
        let d = x * 4;
        dst[d] = rr;
        dst[d + 1] = gg;
        dst[d + 2] = bb;
        dst[d + 3] = aa;
    }
}

/// 24 位：BGR 三字节。
fn decode_24(src: &[u8], width: u32, dst: &mut [u8]) {
    for x in 0..width as usize {
        let o = x * 3;
        let d = x * 4;
        dst[d] = src[o + 2];
        dst[d + 1] = src[o + 1];
        dst[d + 2] = src[o];
        dst[d + 3] = 255;
    }
}

/// 16 位：默认 5-5-5（`BI_RGB`）；有掩码时按掩码（常见 5-6-5）。
fn decode_16(src: &[u8], width: u32, masks: Option<(u32, u32, u32, Option<u32>)>, dst: &mut [u8]) {
    let m = masks.unwrap_or((0x7C00, 0x03E0, 0x001F, None)); // XRGB1555
    for x in 0..width as usize {
        let v = u16::from_le_bytes([src[x * 2], src[x * 2 + 1]]) as u32;
        let get = |mask: u32, def: u8| -> u8 {
            match mask_shift(mask) {
                Some((sh, bits)) => scale_field((v & mask) >> sh, bits),
                None => def,
            }
        };
        let d = x * 4;
        dst[d] = get(m.0, 0);
        dst[d + 1] = get(m.1, 0);
        dst[d + 2] = get(m.2, 0);
        dst[d + 3] = 255;
    }
}

/// 索引色：按位深取调色板下标。
fn decode_indexed(src: &[u8], width: u32, bits: u32, palette: &[[u8; 3]], dst: &mut [u8]) {
    let per_byte = (8 / bits) as usize;
    // 掩码用 u32 算：`1u8 << 8` 会溢出 panic（8 位索引色的 bits 正好是 8）
    let mask = ((1u32 << bits) - 1) as u8;
    for x in 0..width as usize {
        let byte = src[x / per_byte];
        let shift = 8 - bits as usize * (x % per_byte + 1);
        let idx = ((byte >> shift) & mask) as usize;
        let rgb = palette.get(idx).copied().unwrap_or([0, 0, 0]);
        let d = x * 4;
        dst[d] = rgb[0];
        dst[d + 1] = rgb[1];
        dst[d + 2] = rgb[2];
        dst[d + 3] = 255;
    }
}

/// `BI_JPEG` / `BI_PNG`：头之后就是完整的 JPEG/PNG 字节流。
fn decode_embedded(data: &[u8], header_size: usize, compression: u32) -> Result<Bitmap, String> {
    let body = data
        .get(header_size..)
        .ok_or("内嵌图像数据不足（头比数据还长）")?;
    let fmt = if compression == BI_PNG {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    let img = image::load_from_memory_with_format(body, fmt)
        .map_err(|e| format!("内嵌图像解码失败: {e}"))?;
    Ok(Bitmap {
        width: img.width(),
        height: img.height(),
        rgba: img.to_rgba8().into_raw(),
    })
}

/// RGBA 位图 → PNG 字节。
pub fn encode_png(bmp: &Bitmap) -> Result<Vec<u8>, String> {
    let buf = image::RgbaImage::from_raw(bmp.width, bmp.height, bmp.rgba.clone())
        .ok_or("RGBA 缓冲尺寸与宽高不符")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 24 位自下而上的 DIB（辅助函数：写头 + 行）。
    fn make_dib24(width: u32, height: u32, top_down: bool, rows_bgr: &[Vec<u8>]) -> Vec<u8> {
        let stride = row_stride(24, width) as usize;
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&(width as i32).to_le_bytes());
        v.extend_from_slice(
            &(if top_down {
                -(height as i32)
            } else {
                height as i32
            })
            .to_le_bytes(),
        );
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&((stride * height as usize) as u32).to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        for r in rows_bgr {
            let mut row = r.clone();
            row.resize(stride, 0); // 4 字节对齐补齐
            v.extend_from_slice(&row);
        }
        v
    }

    fn make_dib32(width: u32, height: u32, top_down: bool, rows_bgra: &[Vec<u8>]) -> Vec<u8> {
        let stride = row_stride(32, width) as usize;
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&(width as i32).to_le_bytes());
        v.extend_from_slice(
            &(if top_down {
                -(height as i32)
            } else {
                height as i32
            })
            .to_le_bytes(),
        );
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&((stride * height as usize) as u32).to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        for r in rows_bgra {
            let mut row = r.clone();
            row.resize(stride, 0);
            v.extend_from_slice(&row);
        }
        v
    }

    #[test]
    fn row_stride_is_4_byte_aligned() {
        // 1 像素 24 位 = 3 字节 → 补齐到 4
        assert_eq!(row_stride(24, 1), 4);
        assert_eq!(row_stride(24, 2), 8);
        assert_eq!(row_stride(24, 3), 12);
        // 32 位天然对齐
        assert_eq!(row_stride(32, 5), 20);
        // 8 位：3 像素 → 4 字节
        assert_eq!(row_stride(8, 3), 4);
        assert_eq!(row_stride(8, 5), 8);
        // 1 位：9 像素 → 2 字节，补齐到 4
        assert_eq!(row_stride(1, 9), 4);
        assert_eq!(row_stride(1, 33), 8);
    }

    #[test]
    fn bottom_up_rows_are_flipped() {
        // 两行：源第 0 行（自下而上时是**底行**）
        // 注意源是 BGR：`[0,0,255]` 是**红**，`[255,0,0]` 是**蓝**
        let dib = make_dib24(
            1,
            2,
            false,
            &[vec![0, 0, 255], vec![255, 0, 0]], // 底行=红, 顶行=蓝
        );
        let b = parse_dib(&dib).unwrap();
        assert_eq!((b.width, b.height), (1, 2));
        // 输出自上而下：第一行应是顶行（蓝）
        assert_eq!(&b.rgba[0..4], &[0, 0, 255, 255], "顶行应为蓝");
        assert_eq!(&b.rgba[4..8], &[255, 0, 0, 255], "底行应为红");
    }

    #[test]
    fn top_down_rows_are_not_flipped() {
        // biHeight 为负 = 自上而下：源第 0 行就是顶行
        let dib = make_dib24(1, 2, true, &[vec![0, 0, 255], vec![255, 0, 0]]);
        let b = parse_dib(&dib).unwrap();
        // 源第 0 行是 BGR [0,0,255] = 红，应原样留在第一行
        assert_eq!(&b.rgba[0..4], &[255, 0, 0, 255], "顶行应为红（不翻转）");
        assert_eq!(&b.rgba[4..8], &[0, 0, 255, 255], "第二行应为蓝");
    }

    #[test]
    fn rgb_channels_are_not_swapped() {
        // 源是 BGR：字节 [0,0,255] 表示红
        let dib = make_dib24(1, 1, true, &[vec![0, 0, 255]]);
        let b = parse_dib(&dib).unwrap();
        assert_eq!(&b.rgba[0..4], &[255, 0, 0, 255], "BGR 应转成 RGB");
    }

    #[test]
    fn stride_padding_does_not_desync_rows() {
        // 3 像素宽 24 位 = 9 字节 → stride 12（含 3 字节补齐）。
        // 若按 9 字节取行，第二行就会错位——这个用例专门锁住它。
        let dib = make_dib24(
            3,
            2,
            true,
            &[
                vec![0, 0, 255, 0, 0, 255, 0, 0, 255], // 整行红
                vec![255, 0, 0, 255, 0, 0, 255, 0, 0], // 整行蓝
            ],
        );
        let b = parse_dib(&dib).unwrap();
        assert_eq!((b.width, b.height), (3, 2));
        // 第一行三个像素都是红
        for x in 0..3 {
            assert_eq!(
                &b.rgba[x * 4..x * 4 + 4],
                &[255, 0, 0, 255],
                "第 1 行第 {x} 像素"
            );
        }
        // 第二行三个像素都是蓝
        for x in 0..3 {
            let o = (3 + x) * 4;
            assert_eq!(&b.rgba[o..o + 4], &[0, 0, 255, 255], "第 2 行第 {x} 像素");
        }
    }

    #[test]
    fn dib32_zero_alpha_is_treated_as_opaque() {
        // 关键：经典 BI_RGB 的 32 位图第 4 字节常是 0（"未使用"），
        // 当成 alpha 会让整张图全透明——这是最隐蔽的错法之一。
        let dib = make_dib32(1, 1, true, &[vec![0, 0, 255, 0]]);
        let b = parse_dib(&dib).unwrap();
        assert_eq!(&b.rgba[0..4], &[255, 0, 0, 255], "alpha 必须补成 255");
    }

    #[test]
    fn dib32_bgra_order() {
        let dib = make_dib32(1, 1, true, &[vec![10, 20, 30, 255]]);
        let b = parse_dib(&dib).unwrap();
        assert_eq!(&b.rgba[0..4], &[30, 20, 10, 255], "BGRA → RGBA");
    }

    #[test]
    fn bitfields_masks_are_honored() {
        // 16 位 5-6-5 位域：纯红 = 0xF800
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes()); // width
        v.extend_from_slice(&(-1i32).to_le_bytes()); // top-down
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        // 三个掩码（头后紧跟）
        v.extend_from_slice(&0xF800u32.to_le_bytes()); // R
        v.extend_from_slice(&0x07E0u32.to_le_bytes()); // G
        v.extend_from_slice(&0x001Fu32.to_le_bytes()); // B
                                                       // 像素：0xF800（纯红），行步长 4（16 位 × 1 像素 = 2 字节 → 补齐到 4）
        v.extend_from_slice(&0xF800u16.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());

        let b = parse_dib(&v).unwrap();
        assert_eq!(b.rgba[0], 255, "红分量应满");
        assert_eq!(b.rgba[1], 0);
        assert_eq!(b.rgba[2], 0);
        assert_eq!(b.rgba[3], 255);
    }

    #[test]
    fn bitfields_masks_are_not_read_as_pixels() {
        // 掩码占 12 字节：若忘记后移，调色板/像素起点就错了 → 会解出错误颜色
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes());
        v.extend_from_slice(&(-1i32).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0xF800u32.to_le_bytes());
        v.extend_from_slice(&0x07E0u32.to_le_bytes());
        v.extend_from_slice(&0x001Fu32.to_le_bytes());
        // 纯蓝 0x001F
        v.extend_from_slice(&0x001Fu16.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        let b = parse_dib(&v).unwrap();
        assert_eq!(&b.rgba[0..4], &[0, 0, 255, 255], "应为纯蓝");
    }

    #[test]
    fn indexed_8bit_uses_palette() {
        // 调色板两项：红、绿；像素取第 1 项（绿）
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes());
        v.extend_from_slice(&(-1i32).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2u32.to_le_bytes()); // clrUsed = 2
        v.extend_from_slice(&0u32.to_le_bytes());
        // 调色板：BGRX
        v.extend_from_slice(&[0, 0, 255, 0]); // 红
        v.extend_from_slice(&[0, 255, 0, 0]); // 绿
                                              // 像素：下标 1（绿），行步长 4
        v.extend_from_slice(&[1, 0, 0, 0]);

        let b = parse_dib(&v).unwrap();
        assert_eq!(&b.rgba[0..4], &[0, 255, 0, 255], "应取调色板第 1 项（绿）");
    }

    #[test]
    fn indexed_1bit_bit_order_is_msb_first() {
        // 1 位图：每像素 1 bit，**高位在前**。0b1010_0000 → 前四像素 1,0,1,0
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&4i32.to_le_bytes());
        v.extend_from_slice(&(-1i32).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2u32.to_le_bytes()); // clrUsed
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]); // 下标 0 = 黑
        v.extend_from_slice(&[255, 255, 255, 0]); // 下标 1 = 白
        v.extend_from_slice(&[0b1010_0000, 0, 0, 0]);

        let b = parse_dib(&v).unwrap();
        let white = [255u8, 255, 255, 255];
        let black = [0u8, 0, 0, 255];
        assert_eq!(&b.rgba[0..4], &white, "像素0 应为白（最高位）");
        assert_eq!(&b.rgba[4..8], &black, "像素1 应为黑");
        assert_eq!(&b.rgba[8..12], &white, "像素2 应为白");
        assert_eq!(&b.rgba[12..16], &black, "像素3 应为黑");
    }

    #[test]
    fn rejects_rle_and_reports_why() {
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&BI_RLE8.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        let e = parse_dib(&v).unwrap_err();
        assert!(e.contains("RLE"), "{e}");
    }

    #[test]
    fn rejects_short_and_bogus_headers() {
        assert!(parse_dib(&[]).is_err());
        assert!(parse_dib(&[1, 2, 3]).is_err());
        // 声明 12 字节头（BITMAPCOREHEADER）→ 明确不支持
        let mut v = vec![12, 0, 0, 0];
        v.extend_from_slice(&[0u8; 40]);
        let e = parse_dib(&v).unwrap_err();
        assert!(e.contains("不支持的 DIB 头"), "{e}");
    }

    #[test]
    fn rejects_zero_or_negative_dimensions() {
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes()); // width 0
        v.extend_from_slice(&5i32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&[0u8; 16]);
        assert!(parse_dib(&v).unwrap_err().contains("尺寸非法"));
    }

    #[test]
    fn rejects_absurd_dimensions_before_allocating() {
        // 头部声明 100000×100000（100 亿像素）→ 必须在分配前拒绝，否则 OOM
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&100_000i32.to_le_bytes());
        v.extend_from_slice(&100_000i32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&BI_RGB.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&[0u8; 16]);
        let e = parse_dib(&v).unwrap_err();
        assert!(e.contains("过大"), "{e}");
    }

    #[test]
    fn truncated_pixel_data_is_rejected_not_padded() {
        // 声称 4×4 但只给一行像素 → 必须报错而不是补零（补零会得到"有一半是黑"的错图）
        let dib = make_dib24(
            4,
            4,
            true,
            &[vec![0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255]],
        );
        let e = parse_dib(&dib).unwrap_err();
        assert!(e.contains("像素数据不足"), "{e}");
    }

    #[test]
    fn png_round_trip() {
        // DIB → Bitmap → PNG → 再解码回来，颜色与尺寸都要一致。
        // 源按 BGR 写：`[0,0,255]`=红、`[0,255,0]`=绿、`[255,0,0]`=蓝
        let dib = make_dib24(
            2,
            2,
            true,
            &[vec![0, 0, 255, 0, 255, 0], vec![255, 0, 0, 0, 0, 255]],
        );
        let b = parse_dib(&dib).unwrap();
        let png = encode_png(&b).unwrap();
        let back = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((back.width(), back.height()), (2, 2));
        let rgba = back.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 255], "左上应为红");
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 255, 0, 255], "右上应为绿");
        // 第二行第一像素是 BGR [255,0,0] = 蓝
        assert_eq!(rgba.get_pixel(0, 1).0, [0, 0, 255, 255], "左下应为蓝");
    }

    #[test]
    fn our_own_png_to_dib_output_can_be_parsed_back() {
        // 与写回路径的往返：png_to_dib 产出 32 位 BI_RGB 自下而上，
        // 我们的解析器必须能读回来（两处格式约定必须一致）
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let dib = crate::clipboard_write::png_to_dib(&png.into_inner()).unwrap();
        let b = parse_dib(&dib).unwrap();
        assert_eq!((b.width, b.height), (3, 2));
        for i in 0..6 {
            assert_eq!(
                &b.rgba[i * 4..i * 4 + 4],
                &[10, 20, 30, 255],
                "第 {i} 个像素应与原图一致"
            );
        }
    }

    #[test]
    fn tiny_bitmaps_are_not_worth_capturing() {
        let small = Bitmap {
            width: 8,
            height: 8,
            rgba: vec![0; 8 * 8 * 4],
        };
        assert!(!small.is_worth_capturing(), "图标尺寸不值得入库");
        let ok = Bitmap {
            width: 32,
            height: 32,
            rgba: vec![0; 32 * 32 * 4],
        };
        assert!(ok.is_worth_capturing());
        assert_eq!(ok.pixels(), 1024);
    }

    #[test]
    fn mask_helpers() {
        assert_eq!(mask_shift(0xF800), Some((11, 5)));
        assert_eq!(mask_shift(0x07E0), Some((5, 6)));
        assert_eq!(mask_shift(0x001F), Some((0, 5)));
        assert_eq!(mask_shift(0), None);
        // 拉伸：5 位满值 → 255
        assert_eq!(scale_field(31, 5), 255);
        assert_eq!(scale_field(0, 5), 0);
        // 6 位满值 → 255
        assert_eq!(scale_field(63, 6), 255);
        // 中间值四舍五入
        assert_eq!(scale_field(16, 5), 132);
        // 回归：bits=32 曾使 `1u32 << 32` 在 debug 构建 panic
        assert_eq!(scale_field(0xFFFF_FFFF, 32), 255);
        assert_eq!(scale_field(0, 32), 0);
        // 回归：非连续掩码下 value 可远超 2^bits，`value * 255` 曾溢出 u32
        // （debug panic，release 错色）——夹紧到 255 而不是溢出回绕
        assert_eq!(scale_field(0x8000_0001, 2), 255);
        // 超宽掩码但值不成比例时按比例产出（25 位掩码、值 ≈ 100/255 满度）
        assert_eq!(scale_field(13_160_169, 25), 100);
    }

    /// 造一个带自定义掩码的 32 位 BI_BITFIELDS 头 + 单像素。
    fn bitfields32_dib(mr: u32, mg: u32, mb: u32, pixel: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes());
        v.extend_from_slice(&(-1i32).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&mr.to_le_bytes());
        v.extend_from_slice(&mg.to_le_bytes());
        v.extend_from_slice(&mb.to_le_bytes());
        v.extend_from_slice(&pixel.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn bitfields_hostile_masks_do_not_panic() {
        // 回归：剪贴板里的掩码是不可信输入。全 1 的 R 掩码（bits=32）与
        // 非连续掩码曾让 scale_field 在 debug 构建溢出 panic——监听线程当场死掉，
        // 图片捕获静默失效。修复后必须产出夹紧的合法像素。
        let b = parse_dib(&bitfields32_dib(
            0xFFFF_FFFF,
            0x0000_00FF,
            0x0000_FF00,
            0xFFFF_FFFF,
        ))
        .unwrap();
        assert_eq!(
            &b.rgba[0..4],
            &[255, 255, 255, 255],
            "32 位全 1 掩码：满值像素应产出满值颜色（不 panic 不回绕）"
        );
        let b2 = parse_dib(&bitfields32_dib(
            0x8000_0001,
            0x0000_00FF,
            0x0000_FF00,
            0x8000_0001,
        ))
        .unwrap();
        assert_eq!(
            b2.rgba[0], 255,
            "非连续掩码的高位值应夹紧到满（乘法不回绕成小值）"
        );
    }

    #[test]
    fn embedded_png_starts_at_bisize_not_40() {
        // 回归：BI_PNG 的图像流起点在 biSize 之后。V4 头（108 字节）下按写死的
        // 40 切会把头部余下 68 字节混进 PNG 流，解码必败——图片捕获悄悄失效。
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let header_size = 108usize; // BITMAPV4HEADER
        let mut v = vec![0u8; header_size];
        v[0..4].copy_from_slice(&(header_size as u32).to_le_bytes());
        v[4..8].copy_from_slice(&1i32.to_le_bytes()); // width
        v[8..12].copy_from_slice(&1i32.to_le_bytes()); // height
        v[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
        v[14..16].copy_from_slice(&32u16.to_le_bytes()); // bitcount
        v[16..20].copy_from_slice(&BI_PNG.to_le_bytes()); // compression
        v[20..24].copy_from_slice(&(png.len() as u32).to_le_bytes());
        v.extend_from_slice(&png);
        let b = parse_dib(&v).unwrap();
        assert_eq!((b.width, b.height), (2, 1));
        assert_eq!(&b.rgba[0..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn embedded_png_with_40_header_still_works() {
        // 经典 40 字节头的 BI_PNG 不能因修复而回退
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 8, 7, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes());
        v.extend_from_slice(&1i32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&BI_PNG.to_le_bytes());
        v.extend_from_slice(&(png.len() as u32).to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&png);
        let b = parse_dib(&v).unwrap();
        assert_eq!((b.width, b.height), (1, 1));
        assert_eq!(&b.rgba[0..4], &[9, 8, 7, 255]);
    }
}
