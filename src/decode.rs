//! 画像バイト列 → RGBA（p0-6b）。GUI 非依存で、窓を開かずに試験できる。
//!
//! `imgcheck::check` を通ったバイトだけを復号する。壊れたバイトは `Err` で返し、panic しない。
//! 巨大画像（画素数が [`MAX_PIXELS`] 超）は復号前にヘッダの寸法で断る。

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};

use crate::imgcheck::{self, Kind};

/// 受け付ける最大画素数（100 MP）。
pub const MAX_PIXELS: u64 = 100_000_000;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("画像が壊れています: {0}")]
    Invalid(&'static str),
    #[error("画像が大きすぎます: {width}x{height}")]
    TooLarge { width: u32, height: u32 },
    #[error("復号できません: {0}")]
    Decode(String),
}

/// 復号結果。`rgba` は 1 画素 4 バイト（非乗算アルファ）で `width * height * 4` 長。
#[derive(Debug)]
pub struct Decoded {
    pub kind: Kind,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn format_of(kind: Kind) -> ImageFormat {
    match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Jpeg => ImageFormat::Jpeg,
        Kind::Gif => ImageFormat::Gif,
        Kind::Webp => ImageFormat::WebP,
    }
}

/// 検証 → 寸法確認 → 復号。形式は拡張子ではなく magic で決める。
pub fn decode_rgba(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let kind = imgcheck::check(bytes).map_err(DecodeError::Invalid)?;
    let format = format_of(kind);

    let reader = |limits: Option<Limits>| {
        let mut r = ImageReader::with_format(Cursor::new(bytes), format);
        if let Some(limits) = limits {
            r.limits(limits);
        }
        r
    };

    let (width, height) = reader(None)
        .into_dimensions()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(DecodeError::TooLarge { width, height });
    }

    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4 + (64 << 20));
    let img = reader(Some(limits))
        .decode()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
    let rgba = img.into_rgba8();
    Ok(Decoded {
        kind,
        width,
        height,
        rgba: rgba.into_raw(),
    })
}
