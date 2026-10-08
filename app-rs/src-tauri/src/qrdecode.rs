//! 二维码识别：图片字节 → 解码文本，以及 WIFI: 串的结构化解析。
//!
//! 生成侧在 `wifi.rs` / commands（qrcode crate）；本模块是反方向——
//! 截图/聊天图片里的二维码 → 原文。解码用 rqrr（纯 Rust，走 image 的
//! Luma 灰度缓冲）；WIFI: 解析是 `wifi.rs::escape_wifi_field` 的对偶
//! （转义还原），单测两侧互锁。
//!
//! 图片来源有三条入口（都汇聚到 `decode_image_bytes`）：
//! - 工具箱「二维码识别」tab：选文件（前端读 bytes）/ 读剪贴板 CF_DIB
//! - 剪贴板历史图片条目 ··· 菜单：读 clipboard_files 里已存的 PNG

use image::GrayImage;
use rqrr::PreparedImage;

/// 解码图片字节（png/jpeg 等常见格式）中的二维码，返回首个码的文本。
///
/// 多码图片取第一个识别结果（rqrr 无置信度排序，这里不假装能挑"最对"的）。
pub fn decode_image_bytes(data: &[u8]) -> Result<String, String> {
    let img = image::load_from_memory(data).map_err(|e| format!("图片解码失败: {e}"))?;
    decode_gray(&img.to_luma8())
}

/// 从灰度图解码（公开给测试：round-trip 直接喂生成的 Luma，不经编码器）。
pub fn decode_gray(gray: &GrayImage) -> Result<String, String> {
    let mut prepared = PreparedImage::prepare(gray.clone());
    let codes = prepared.detect_grids();
    let first = codes
        .into_iter()
        .next()
        .ok_or_else(|| "未在图片中识别到二维码".to_string())?;
    first
        .decode()
        .map(|c| c.1)
        .map_err(|e| format!("二维码解码失败: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrcode::QrCode;

    /// 生成→解码 round-trip：qrcode 渲染灰度 → rqrr 识别。
    /// 这是解码器可用性的主锁（不需要外部测试图片）。
    #[test]
    fn round_trip_ascii() {
        let code = QrCode::new("https://example.com/syskit?qr=1").unwrap();
        let img = code
            .render::<image::Luma<u8>>()
            .min_dimensions(200, 200)
            .dark_color(image::Luma([0u8]))
            .light_color(image::Luma([255u8]))
            .build();
        assert_eq!(
            decode_gray(&img).unwrap(),
            "https://example.com/syskit?qr=1"
        );
    }

    #[test]
    fn round_trip_chinese_and_escaped_password() {
        // 中文内容 + WIFI 串（密码带分号转义）——UTF-8 与转义还原一起锁
        let text = "WIFI:T:WPA;S:Home\\;5G;P:p@ss\\;word;;";
        let code = QrCode::new(text.as_bytes()).unwrap();
        let img = code
            .render::<image::Luma<u8>>()
            .min_dimensions(240, 240)
            .build();
        assert_eq!(decode_gray(&img).unwrap(), text);
    }

    #[test]
    fn no_qr_is_clear_error() {
        // 纯白图：识别不到 → 明确错误文案而不是 panic
        let img = image::GrayImage::from_pixel(200, 200, image::Luma([255u8]));
        let err = decode_gray(&img).unwrap_err();
        assert!(err.contains("识别到二维码"), "{err}");
    }
}
