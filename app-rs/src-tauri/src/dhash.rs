//! 图片感知哈希（dHash 9×8 差分）：截图工具每次重编码 md5 都不同，字节级
//! 去重抓不住「同一张图又截了一遍」——感知哈希按**结构相似度**判重。
//!
//! 算法：缩到 9×8 灰度 → 每行左像素 < 右像素 → 64 位指纹；两张图的指纹
//! 汉明距离 ≤5（64 位里 ≤8% 位不同）视为同一张图。阈值 5 是经验值：
//! 同图重编码/轻微压缩噪声距离 0~3，不同截图距离通常 >20。
//!
//! 纯逻辑模块（image 解码 + 缩放），无 Win32。

use image::imageops::FilterType;

/// 计算 PNG/JPEG 字节的 dHash 指纹；图片解码失败返回 None（调用方降级为
/// 不做感知查重，不影响入库）。
pub fn dhash64(png: &[u8]) -> Option<u64> {
    let img = image::load_from_memory(png).ok()?;
    Some(dhash_gray(&img.to_luma8()))
}

/// 灰度图 → dHash（公开给测试：round-trip 直接喂生成图）。
pub fn dhash_gray(gray: &image::GrayImage) -> u64 {
    let scaled = image::imageops::resize(gray, 9, 8, FilterType::Triangle);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            // 行内左 < 右 → 置位（高位在左：x=0 是最高位）
            if scaled.get_pixel(x, y).0[0] < scaled.get_pixel(x + 1, y).0[0] {
                hash |= 1u64 << (63 - (y * 8 + x));
            }
        }
    }
    hash
}

/// 汉明距离（指纹不同位的数量）。
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 判重阈值：距离 ≤5 视为同一张图。
pub const DUPLICATE_DISTANCE: u32 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn solid_gray(w: u32, h: u32, v: u8) -> image::GrayImage {
        image::GrayImage::from_pixel(w, h, Luma([v]))
    }

    #[test]
    fn same_image_same_hash() {
        let img = solid_gray(64, 64, 128);
        assert_eq!(dhash_gray(&img), dhash_gray(&img));
    }

    #[test]
    fn horizontal_gradient_is_all_ones() {
        // 每行左暗右亮：全部 64 位都置位
        let mut img = solid_gray(90, 80, 255);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = Luma([(x / 5) as u8]); // 0..=17 递增（9 列采样点）
        }
        assert_eq!(dhash_gray(&img), u64::MAX);
    }

    #[test]
    fn reencode_same_screenshot_small_distance() {
        // 模拟重编码：同内容图轻微加噪（±3）——距离应 ≤5
        let base = solid_gray(120, 90, 100);
        let mut noisy = base.clone();
        for (x, y, p) in noisy.enumerate_pixels_mut() {
            if (x + y) % 17 == 0 {
                p.0[0] = p.0[0].saturating_add(3);
            }
        }
        let d = hamming(dhash_gray(&base), dhash_gray(&noisy));
        assert!(d <= DUPLICATE_DISTANCE, "重编码噪声距离 {d} 应 ≤5");
    }

    #[test]
    fn different_images_large_distance() {
        // 纯色图指纹恒全 0（无梯度）——两张空白截图判重恰是期望行为；
        // 「不同图」必须有结构差异：左右渐变 vs 上下渐变
        let mut lr = solid_gray(120, 90, 0);
        for (x, _y, p) in lr.enumerate_pixels_mut() {
            p.0[0] = if x < 60 { 0 } else { 255 };
        }
        let mut ud = solid_gray(120, 90, 0);
        for (_x, y, p) in ud.enumerate_pixels_mut() {
            p.0[0] = if y < 45 { 0 } else { 255 };
        }
        let d = hamming(dhash_gray(&lr), dhash_gray(&ud));
        // Triangle 插值会模糊硬边界（实测 16），仍远大于判重阈值 5
        assert!(d > 10, "左右渐变 vs 上下渐变 距离 {d} 应远大于阈值");
    }

    #[test]
    fn solid_images_share_hash_by_design() {
        // 纯色图（两张空白截图）指纹相同 → 判重成立（期望行为，非缺陷）
        assert_eq!(
            dhash_gray(&solid_gray(64, 64, 0)),
            dhash_gray(&solid_gray(64, 64, 255))
        );
    }

    #[test]
    fn dhash64_png_round_trip() {
        // 生成 PNG → 解码 → 指纹；两次编码同一内容指纹一致
        let img = solid_gray(120, 90, 90);
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        let mut img2 = img.clone();
        for (_x, _y, p) in img2.enumerate_pixels_mut() {
            p.0[0] = p.0[0].wrapping_add(1);
        }
        let mut out2 = Vec::new();
        img2.write_to(
            &mut std::io::Cursor::new(&mut out2),
            image::ImageFormat::Png,
        )
        .unwrap();
        let h1 = dhash64(&out).unwrap();
        let h2 = dhash64(&out2).unwrap();
        assert!(hamming(h1, h2) <= DUPLICATE_DISTANCE);
        // 乱数据返回 None（不 panic）
        assert!(dhash64(b"not an image").is_none());
    }
}
