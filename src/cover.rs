//! 表紙サムネイル（設計 p0-2c、#2）。GUI 非依存。
//!
//! 表紙 = 頁順の先頭。[`MAX_W`]×[`MAX_H`] に収まるよう縮小（拡大はしない）して
//! JPEG（品質 [`QUALITY`]）にする。壊れた先頭頁は `decode_rgba` の修復に任せ、
//! それでも読めなければ `Err`（呼び手は表紙なしで続ける）。

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbaImage};

use crate::book::Book;
use crate::decode::decode_rgba;

/// 表紙の最大幅（設計 §2.2）。
pub const MAX_W: u32 = 240;
/// 表紙の最大高さ（設計 §2.2）。
pub const MAX_H: u32 = 340;
/// JPEG 品質（設計 §2.2）。
pub const QUALITY: u8 = 80;

/// 作った表紙。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}

/// 表紙を作れなかった理由。
#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("先頭頁を読めません: {0}")]
    Read(String),
    #[error("先頭頁を復号できません: {0}")]
    Decode(String),
    #[error("JPEG にできません: {0}")]
    Encode(String),
}

/// 冊の先頭頁から表紙を作る。
pub fn make_cover(book: &Book) -> Result<Cover, CoverError> {
    let bytes = book
        .read_page_bytes(0)
        .map_err(|e| CoverError::Read(e.to_string()))?;
    cover_from_bytes(&bytes)
}

/// 画像バイト列から表紙を作る。
pub fn cover_from_bytes(bytes: &[u8]) -> Result<Cover, CoverError> {
    let decoded = decode_rgba(bytes).map_err(|e| CoverError::Decode(e.to_string()))?;
    let rgba = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| CoverError::Decode("画素数が寸法と合いません".to_string()))?;
    let (width, height) = fit(decoded.width, decoded.height);
    let small = if (width, height) == (decoded.width, decoded.height) {
        rgba
    } else {
        imageops::resize(&rgba, width, height, FilterType::Triangle)
    };
    // JPEG はアルファを持てないので RGB にする
    let rgb = DynamicImage::ImageRgba8(small).into_rgb8();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode_image(&rgb)
        .map_err(|e| CoverError::Encode(e.to_string()))?;
    Ok(Cover {
        width,
        height,
        jpeg,
    })
}

/// アスペクト比を保って [`MAX_W`]×[`MAX_H`] に収まる寸法（拡大しない、最小 1）。
#[must_use]
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_W && height <= MAX_H {
        return (width.max(1), height.max(1));
    }
    let (w, h) = (u64::from(width), u64::from(height));
    // 幅と高さのうち、きつい方の比で縮める
    if w * u64::from(MAX_H) >= h * u64::from(MAX_W) {
        let nh = (h * u64::from(MAX_W) + w / 2) / w;
        (MAX_W, u32::try_from(nh).unwrap_or(MAX_H).clamp(1, MAX_H))
    } else {
        let nw = (w * u64::from(MAX_H) + h / 2) / h;
        (u32::try_from(nw).unwrap_or(MAX_W).clamp(1, MAX_W), MAX_H)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_small_images_and_aspect() {
        assert_eq!(fit(100, 200), (100, 200));
        assert_eq!(fit(240, 340), (240, 340));
        // 縦長: 高さで決まる
        assert_eq!(fit(1400, 2000), (238, 340));
        // 横長（見開き）: 幅で決まる
        assert_eq!(fit(3000, 2000), (240, 160));
        // 極端に細い画像でも 0 にならない
        assert_eq!(fit(100_000, 10), (240, 1));
        assert_eq!(fit(10, 100_000), (1, 340));
    }
}
