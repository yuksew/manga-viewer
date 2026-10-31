//! 画像バイト列 → RGBA（p0-6b）。GUI 非依存で、窓を開かずに試験できる。
//!
//! `imgcheck::check` を通ったバイトだけを復号する。壊れたバイトは `Err` で返し、panic しない。
//! 巨大画像（画素数が [`MAX_PIXELS`] 超）は復号前にヘッダの寸法で断る。
//!
//! 厳密に復号できなかった PNG/JPEG などは [`Repair`] の順で救済を試み、救済できたときは
//! [`Decoded::repair`] に印を残す（呼び出し側が利用者に示す。静かに化かさない）。

use std::io::Cursor;

use fast_image_resize::{self as fir, ResizeOptions, Resizer};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

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

/// 修復の種類。`Decoded::repair` が `Some` なら、元データのままでは厳密に復号できなかった。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repair {
    /// 終端マーカー（JPEG の EOI / PNG の IEND）の後ろのゴミを捨てた。画素は欠けていない。
    /// 先頭 `len` バイトが有効な画像。
    TrailingGarbage { len: usize },
    /// 末尾が欠けた本体を、復号できるところまで読んだ（欠けた部分は画像に依存した穴埋め）。
    Truncated,
}

impl Repair {
    /// 利用者向けの注記。
    #[must_use]
    pub fn note(self) -> &'static str {
        match self {
            Repair::TrailingGarbage { .. } => {
                "元データの末尾に余分なデータがあり、取り除いて表示しています"
            }
            Repair::Truncated => "元データが途中で切れていたため、読めた範囲だけ表示しています",
        }
    }
}

/// 復号結果。`rgba` は 1 画素 4 バイト（非乗算アルファ）で `width * height * 4` 長。
#[derive(Debug)]
pub struct Decoded {
    pub kind: Kind,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// 修復して得た結果なら、その種類
    pub repair: Option<Repair>,
    /// 元画像にアルファが無い（全画素が不透明）。乗算済みと非乗算が同じなので、表示側は変換を省ける
    pub opaque: bool,
    /// 縮小前の元画像の寸法（[`decode_fit`] で縮めたときも元の大きさ）
    pub source_size: (u32, u32),
}

/// 復号した画像（形式は元のまま。RGBA 化・縮小の前）。
struct Raw {
    kind: Kind,
    img: DynamicImage,
    repair: Option<Repair>,
}

fn format_of(kind: Kind) -> ImageFormat {
    match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Jpeg => ImageFormat::Jpeg,
        Kind::Gif => ImageFormat::Gif,
        Kind::Webp => ImageFormat::WebP,
    }
}

/// 先頭の magic だけで形式を決める（終端の整合は見ない）。
fn sniff(b: &[u8]) -> Option<Kind> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if b.starts_with(&[0xFF, 0xD8]) {
        Some(Kind::Jpeg)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else {
        None
    }
}

/// 検証 → 寸法確認 → 復号。形式は拡張子ではなく magic で決める。
/// 厳密に読めなければ修復を試み、成功時は [`Decoded::repair`] に印を付ける。
pub fn decode_rgba(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let raw = decode_image(bytes)?;
    let (width, height) = (raw.img.width(), raw.img.height());
    let opaque = !raw.img.color().has_alpha();
    Ok(Decoded {
        kind: raw.kind,
        width,
        height,
        rgba: raw.img.into_rgba8().into_raw(),
        repair: raw.repair,
        opaque,
        source_size: (width, height),
    })
}

/// [`decode_rgba`] と同じ検証・修復で復号し、`max_w`×`max_h` に収まるよう縮小して RGBA にする（#8）。
///
/// 収まっていれば縮小しない（拡大もしない）。縮小は SIMD（`fast_image_resize`）で、元の画素形式
/// （グレー / RGB / RGBA）のまま縮めてから RGBA にするので、全画素の変換は縮小後の 1 回だけ。
pub fn decode_fit(bytes: &[u8], max_w: u32, max_h: u32) -> Result<Decoded, DecodeError> {
    let raw = decode_image(bytes)?;
    let source_size = (raw.img.width(), raw.img.height());
    let opaque = !raw.img.color().has_alpha();
    let (w, h) = fit_within(source_size, (max_w, max_h));
    let img = if (w, h) == source_size {
        raw.img
    } else {
        resize(raw.img, w, h)?
    };
    Ok(Decoded {
        kind: raw.kind,
        width: w,
        height: h,
        rgba: img.into_rgba8().into_raw(),
        repair: raw.repair,
        opaque,
        source_size,
    })
}

/// 縦横比を保って `max` に収まる寸法（拡大しない・最小 1）。
#[must_use]
pub fn fit_within((w, h): (u32, u32), (max_w, max_h): (u32, u32)) -> (u32, u32) {
    let (max_w, max_h) = (max_w.max(1), max_h.max(1));
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let scale = (f64::from(max_w) / f64::from(w)).min(f64::from(max_h) / f64::from(h));
    let fit = |v: u32, max: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max);
    (fit(w, max_w), fit(h, max_h))
}

/// 元の画素形式のまま SIMD で縮める。16 bit などの珍しい形式は 8 bit にしてから縮める。
fn resize(img: DynamicImage, w: u32, h: u32) -> Result<DynamicImage, DecodeError> {
    let img = match img {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => {
            img
        }
        other if other.color().has_alpha() => DynamicImage::ImageRgba8(other.into_rgba8()),
        other => DynamicImage::ImageRgb8(other.into_rgb8()),
    };
    let mut dst = DynamicImage::new(w, h, img.color());
    let options =
        ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear));
    Resizer::new()
        .resize(&img, &mut dst, &options)
        .map_err(|e| DecodeError::Decode(format!("縮小できません: {e}")))?;
    Ok(dst)
}

/// 検証 → 寸法確認 → 復号（必要なら修復）。RGBA 化はしない。
fn decode_image(bytes: &[u8]) -> Result<Raw, DecodeError> {
    let strict_err = match decode_strict(bytes) {
        Ok(d) => return Ok(d),
        Err(e) => e,
    };
    let Some(kind) = sniff(bytes) else {
        return Err(strict_err);
    };
    // 巨大画像は修復の対象にしない
    if matches!(strict_err, DecodeError::TooLarge { .. }) {
        return Err(strict_err);
    }

    // 1) 終端マーカーで切る（正確に直せるので、読める所まで読む救済より先に試す）
    for len in end_candidates(kind, bytes) {
        if let Ok(mut d) = decode_strict(&bytes[..len]) {
            d.repair = Some(Repair::TrailingGarbage { len });
            return Ok(d);
        }
    }
    // 2) 末尾欠けのまま、読めるところまで復号（imgcheck の終端検査を外す）
    if let Ok(mut d) = decode_raw(kind, bytes) {
        d.repair = Some(Repair::Truncated);
        return Ok(d);
    }
    Err(strict_err)
}

/// 終端マーカー（JPEG: `FFD9`、PNG: `IEND` チャンク末尾）の直後で切る長さの候補。前から最大 16 個。
///
/// 前から試す理由（p0-8c / 所見 F1）: 真の終端より後ろにあるのは末尾ゴミだけなので、最初に
/// strict 復号が通る位置を採ればゴミ中のマーカーに引きずられない。JPEG の走査データ内に
/// `FFD9` は現れない（`FF` は `FF00`/RSTn にスタッフされる）ため、前方の偽の候補は EXIF
/// サムネイル等のヘッダ内だけで、そこで切った本体は strict 検証で落ちる（試行が数回増えるだけ）。
fn end_candidates(kind: Kind, b: &[u8]) -> Vec<usize> {
    const MAX: usize = 16;
    let (marker, extra): (&[u8], usize) = match kind {
        Kind::Jpeg => (&[0xFF, 0xD9], 2),
        Kind::Png => (b"IEND", 8), // 種別 4 + CRC 4
        _ => return Vec::new(),
    };
    b.windows(marker.len())
        .enumerate()
        .filter(|(_, w)| *w == marker)
        .map(|(i, _)| i + extra)
        .filter(|&end| end < b.len()) // 全長で切るのは元と同じなので除く
        .take(MAX)
        .collect()
}

fn decode_strict(bytes: &[u8]) -> Result<Raw, DecodeError> {
    let kind = imgcheck::check(bytes).map_err(DecodeError::Invalid)?;
    decode_raw(kind, bytes)
}

/// 寸法確認 → 復号（`imgcheck` は通さない）。
fn decode_raw(kind: Kind, bytes: &[u8]) -> Result<Raw, DecodeError> {
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
    Ok(Raw {
        kind,
        img,
        repair: None,
    })
}
