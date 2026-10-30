//! 画像バイト列の軽い検証（p0-4）。デコードはせず、先頭の magic とヘッダ・末尾の整合だけを見る。
//!
//! 見るもの（std のみ）:
//! * PNG — 署名、先頭が IHDR（幅・高さ非 0）、チャンク長を辿って `IEND` で**ちょうど**終わる
//! * JPEG — `FFD8` 始まり、SOS より前に SOF（幅・高さ非 0）、末尾 `FFD9`（後ろの 0 埋めは許す）
//! * GIF — `GIF87a`/`GIF89a`、幅・高さ非 0、末尾 `3B`
//! * WebP — `RIFF` + サイズ + `WEBP`、先頭チャンクが `VP8 `/`VP8L`/`VP8X`、RIFF サイズが実長を超えない
//!
//! 見ないもの（限界）: 圧縮データの中身、PNG の CRC、JPEG のスキャン内部。
//! ヘッダと終端が正しいまま中身だけ壊れた画像は通る。トランケイト（途中切れ）は捕まえる。

/// 検証済みの画像形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl Kind {
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Gif => "image/gif",
            Kind::Webp => "image/webp",
        }
    }

    #[must_use]
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Jpeg => "jpg",
            Kind::Gif => "gif",
            Kind::Webp => "webp",
        }
    }
}

/// 形式を magic で判定して妥当性を確かめる。壊れていれば理由つきの `Err`。
pub fn check(b: &[u8]) -> Result<Kind, &'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(b).map(|()| Kind::Png)
    } else if b.starts_with(&[0xFF, 0xD8]) {
        jpeg(b).map(|()| Kind::Jpeg)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        gif(b).map(|()| Kind::Gif)
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        webp(b).map(|()| Kind::Webp)
    } else {
        Err("画像の署名がありません")
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn png(b: &[u8]) -> Result<(), &'static str> {
    // 署名 8 + IHDR（長さ 4 + 種別 4 + 本体 13 + CRC 4）
    if be32(b, 8) != Some(13) || b.get(12..16) != Some(b"IHDR") {
        return Err("PNG の IHDR がありません");
    }
    if b.len() < 33 || be32(b, 16) == Some(0) || be32(b, 20) == Some(0) {
        return Err("PNG の寸法が不正です");
    }
    let mut pos = 8;
    while pos < b.len() {
        let len = be32(b, pos).ok_or("PNG が途中で切れています")? as usize;
        let kind = b.get(pos + 4..pos + 8).ok_or("PNG が途中で切れています")?;
        let end = pos
            .checked_add(12)
            .and_then(|p| p.checked_add(len))
            .filter(|&e| e <= b.len())
            .ok_or("PNG が途中で切れています")?;
        if kind == b"IEND" {
            return if end == b.len() && len == 0 {
                Ok(())
            } else {
                Err("PNG の IEND が不正です")
            };
        }
        pos = end;
    }
    Err("PNG に IEND がありません")
}

fn jpeg(b: &[u8]) -> Result<(), &'static str> {
    let mut end = b.len();
    while end > 0 && b[end - 1] == 0 {
        end -= 1;
    }
    if end < 4 || b[end - 2..end] != [0xFF, 0xD9] {
        return Err("JPEG の EOI がありません");
    }
    let mut pos = 2;
    loop {
        if b.get(pos) != Some(&0xFF) {
            return Err("JPEG のマーカーが不正です");
        }
        // 0xFF の連続（フィル）は読み飛ばす
        while b.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let marker = *b.get(pos + 1).ok_or("JPEG が途中で切れています")?;
        pos += 2;
        match marker {
            0x01 | 0xD0..=0xD7 => continue,
            0xD9 | 0xDA => return Err("JPEG に SOF がありません"),
            _ => {}
        }
        let len = b.get(pos..pos + 2).ok_or("JPEG が途中で切れています")?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        if len < 2 || pos + len > b.len() {
            return Err("JPEG が途中で切れています");
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let seg = &b[pos + 2..pos + len];
            if seg.len() < 6 {
                return Err("JPEG の SOF が短すぎます");
            }
            let h = u16::from_be_bytes([seg[1], seg[2]]);
            let w = u16::from_be_bytes([seg[3], seg[4]]);
            return if h == 0 || w == 0 {
                Err("JPEG の寸法が不正です")
            } else {
                Ok(())
            };
        }
        pos += len;
    }
}

fn gif(b: &[u8]) -> Result<(), &'static str> {
    if b.len() < 14 {
        return Err("GIF が短すぎます");
    }
    let w = u16::from_le_bytes([b[6], b[7]]);
    let h = u16::from_le_bytes([b[8], b[9]]);
    if w == 0 || h == 0 {
        return Err("GIF の寸法が不正です");
    }
    if b[b.len() - 1] != 0x3B {
        return Err("GIF の終端がありません");
    }
    Ok(())
}

fn webp(b: &[u8]) -> Result<(), &'static str> {
    let riff = le32(b, 4).ok_or("WebP が短すぎます")? as usize;
    if riff.saturating_add(8) > b.len() {
        return Err("WebP が途中で切れています");
    }
    match b.get(12..16) {
        Some(b"VP8 " | b"VP8L" | b"VP8X") => {}
        _ => return Err("WebP のチャンクが不正です"),
    }
    let chunk = le32(b, 16).ok_or("WebP が短すぎます")? as usize;
    if chunk.saturating_add(20) > riff.saturating_add(8) {
        return Err("WebP が途中で切れています");
    }
    Ok(())
}
