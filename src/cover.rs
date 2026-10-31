//! 表紙サムネイル（設計 p0-2c、#2）。GUI 非依存。
//!
//! 表紙 = 頁順の先頭。[`MAX_W`]×[`MAX_H`] に収まるよう縮小（拡大はしない）して
//! JPEG（品質 [`QUALITY`]）にする。壊れた先頭頁は `decode_fit`（`decode_rgba` と同じ修復）に任せ、
//! それでも読めなければ `Err`（呼び手は表紙なしで続ける）。

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbaImage};

use crate::book::Book;
use crate::decode::decode_fit;

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

/// 画像バイト列から表紙を作る。縮小は頁表示と同じ SIMD の [`decode_fit`]（#8）。
pub fn cover_from_bytes(bytes: &[u8]) -> Result<Cover, CoverError> {
    let decoded = decode_fit(bytes, MAX_W, MAX_H).map_err(|e| CoverError::Decode(e.to_string()))?;
    let rgba = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| CoverError::Decode("画素数が寸法と合いません".to_string()))?;
    // JPEG はアルファを持てないので RGB にする
    let rgb = DynamicImage::ImageRgba8(rgba).into_rgb8();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode_image(&rgb)
        .map_err(|e| CoverError::Encode(e.to_string()))?;
    Ok(Cover {
        width: decoded.width,
        height: decoded.height,
        jpeg,
    })
}

#[cfg(test)]
mod tests {
    use crate::decode::fit_within;

    use super::{MAX_H, MAX_W};

    #[test]
    fn cover_bounds_keep_aspect() {
        let fit = |w, h| fit_within((w, h), (MAX_W, MAX_H));
        assert_eq!(fit(100, 200), (100, 200));
        assert_eq!(fit(240, 340), (240, 340));
        assert_eq!(fit(1400, 2000), (238, 340));
        assert_eq!(fit(3000, 2000), (240, 160));
        assert_eq!(fit(100_000, 10), (240, 1));
        assert_eq!(fit(10, 100_000), (1, 340));
    }
}
