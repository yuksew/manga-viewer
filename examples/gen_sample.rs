//! 手動確認用の合成漫画データを作る。
//!
//! ```
//! cargo run --release --example gen_sample [-- <出力先>]   # 既定は sample-books/
//! manga-viewer index sample-books
//! ```
//!
//! 頁には大きな頁番号（7 セグ）と冊ごとの色帯を描くので、頁送り・見開きの並び・次冊移動が目で確かめられる。
//! 壊れた頁・壊れた冊・空の冊・横長頁・巨大頁も混ぜてある。
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgb, RgbImage};
use zip::write::SimpleFileOptions;

const W: u32 = 1200;
const H: u32 = 1700;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sample-books"));
    if out.exists() {
        fs::remove_dir_all(&out)?;
    }
    fs::create_dir_all(&out)?;

    // 次冊/前冊の確認用の連作（zip、3 桁ゼロ埋め名）
    for (vol, pages, hue) in [(1, 24, 0), (2, 20, 1), (3, 23, 2)] {
        let entries: Vec<_> = (1..=pages)
            .map(|p| {
                let img = page(p, pages, hue, W, H);
                (format!("{p:03}.jpg"), encode(&img, ImageFormat::Jpeg))
            })
            .collect();
        write_zip(&out.join(format!("シリーズA 第{vol}巻.zip")), &entries)?;
    }

    // 自然順（1.png < 2.png < 10.png）の確認用。cbz
    let entries: Vec<_> = (1..=12)
        .map(|p| {
            (
                format!("{p}.png"),
                encode(&page(p, 12, 3, W, H), ImageFormat::Png),
            )
        })
        .collect();
    write_zip(&out.join("自然順テスト.cbz"), &entries)?;

    // 画像フォルダの冊（奇数頁で見開きの端を確かめる）
    write_folder(&out.join("フォルダ本"), 15, 4)?;
    // 深さ 2 のフォルダ冊（上下巻）
    write_folder(&out.join("作者B").join("短編集 上"), 9, 5)?;
    write_folder(&out.join("作者B").join("短編集 下"), 8, 6)?;

    // 形式の混在 + 画像以外のエントリ + zip 内サブディレクトリ
    let fmts = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
    ];
    let mut entries: Vec<(String, Vec<u8>)> = (1..=8)
        .map(|p| {
            let f = fmts[(p - 1) as usize % fmts.len()];
            let ext = f.extensions_str()[0];
            // GIF の量子化は重いので小さめにする
            let (w, h) = if f == ImageFormat::Gif {
                (600, 850)
            } else {
                (W, H)
            };
            (
                format!("本文/{p:02}.{ext}"),
                encode(&page(p, 8, 7, w, h), f),
            )
        })
        .collect();
    entries.push(("readme.txt".into(), "画像ではないので頁にならない".into()));
    entries.push(("Thumbs.db".into(), vec![0u8; 64]));
    write_zip(&out.join("混在形式.zip"), &entries)?;

    // 壊れた頁を含む冊: 3 頁目は途中切れ、5 頁目は末尾ゴミ、7 頁目は中身がでたらめ
    let entries: Vec<_> = (1..=8)
        .map(|p| {
            let mut b = encode(&page(p, 8, 8, W, H), ImageFormat::Jpeg);
            match p {
                3 => b.truncate(b.len() * 2 / 3),
                5 => b.extend_from_slice(b"GARBAGE-AFTER-EOI\x00\x01\x02"),
                7 => b = noise(4096, 7),
                _ => {}
            }
            (format!("{p:02}.jpg"), b)
        })
        .collect();
    write_zip(&out.join("壊れ頁あり.zip"), &entries)?;

    // 冊ごと壊れている / 画像が 1 枚も無い
    fs::write(out.join("壊れzip.zip"), noise(10_000, 1))?;
    write_zip(
        &out.join("空.zip"),
        &[("memo.txt".into(), b"empty".to_vec())],
    )?;

    // 横長（見開き 1 枚絵）を挟む冊
    let entries: Vec<_> = (1..=10)
        .map(|p| {
            let w = if p == 5 || p == 6 { W * 2 } else { W };
            (
                format!("{p:02}.jpg"),
                encode(&page(p, 10, 9, w, H), ImageFormat::Jpeg),
            )
        })
        .collect();
    write_zip(&out.join("横長頁あり.zip"), &entries)?;

    // 速度確認用の巨大頁
    let entries: Vec<_> = (1..=6)
        .map(|p| {
            (
                format!("{p:02}.jpg"),
                encode(&page(p, 6, 10, 3000, 4250), ImageFormat::Jpeg),
            )
        })
        .collect();
    write_zip(&out.join("大きい頁.zip"), &entries)?;

    println!("生成しました: {}", out.display());
    Ok(())
}

/// 1 頁を描く: 白地・コマ割り・トーン・上の色帯・中央の大きな頁番号・下の「n / total」。
fn page(n: u32, total: u32, hue: usize, w: u32, h: u32) -> RgbImage {
    const COLORS: [[u8; 3]; 11] = [
        [220, 60, 60],
        [60, 140, 220],
        [60, 180, 90],
        [230, 150, 30],
        [150, 80, 200],
        [30, 170, 170],
        [200, 80, 140],
        [120, 120, 40],
        [90, 90, 90],
        [180, 110, 60],
        [40, 70, 160],
    ];
    let mut img = RgbImage::from_pixel(w, h, Rgb([250, 250, 246]));
    let color = Rgb(COLORS[hue % COLORS.len()]);
    let m = w / 24;

    // コマ割り（頁ごとに段の数を変える）
    let rows = 2 + n % 3;
    let top = h / 12;
    let bottom = h - h / 10;
    let row_h = (bottom - top) / rows;
    for r in 0..rows {
        let y0 = top + r * row_h + m / 3;
        let y1 = top + (r + 1) * row_h - m / 3;
        let split = !(r + n).is_multiple_of(3);
        let cols: &[(u32, u32)] = if split {
            let mid = w * (35 + (r * 17 + n * 7) % 30) / 100;
            &[(m, mid - m / 4), (mid + m / 4, w - m)]
        } else {
            &[(m, w - m)]
        };
        for (i, &(x0, x1)) in cols.iter().enumerate() {
            if (r + i as u32 + n).is_multiple_of(4) {
                tone(&mut img, x0, y0, x1, y1, 10);
            }
            frame(
                &mut img,
                x0,
                y0,
                x1,
                y1,
                (w / 300).max(3),
                Rgb([20, 20, 20]),
            );
        }
    }

    // 色帯（冊の区別）と進み具合のバー
    fill(&mut img, 0, 0, w, h / 40, color);
    let prog = w * n / total;
    fill(
        &mut img,
        0,
        h / 40,
        prog,
        h / 40 + h / 120,
        Rgb([40, 40, 40]),
    );

    // 中央の大きな頁番号（白い地の上に）
    let digit_h = h / 5;
    let num = n.to_string();
    let dw = digit_h / 2;
    let gap = dw / 4;
    let tw = num.len() as u32 * (dw + gap) - gap;
    let x = (w - tw.min(w)) / 2;
    let y = (h - digit_h) / 2;
    fill(
        &mut img,
        x.saturating_sub(gap * 2),
        y - gap * 2,
        x + tw + gap * 2,
        y + digit_h + gap * 2,
        Rgb([255, 255, 255]),
    );
    frame(
        &mut img,
        x.saturating_sub(gap * 2),
        y - gap * 2,
        x + tw + gap * 2,
        y + digit_h + gap * 2,
        6,
        color,
    );
    draw_number(&mut img, &num, x, y, digit_h, color);

    // 下の「n / total」
    let small = h / 28;
    let label = format!("{n}/{total}");
    let sw = small / 2;
    let lw = label.len() as u32 * (sw + sw / 4);
    draw_number(
        &mut img,
        &label,
        w - m - lw,
        h - h / 20,
        small,
        Rgb([30, 30, 30]),
    );
    img
}

fn fill(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, c: Rgb<u8>) {
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            img.put_pixel(x, y, c);
        }
    }
}

fn frame(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, t: u32, c: Rgb<u8>) {
    fill(img, x0, y0, x1, y0 + t, c);
    fill(img, x0, y1 - t, x1, y1, c);
    fill(img, x0, y0, x0 + t, y1, c);
    fill(img, x1 - t, y0, x1, y1, c);
}

/// 網点トーン。
fn tone(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, pitch: u32) {
    for y in (y0..y1).step_by(pitch as usize) {
        let off = if (y / pitch).is_multiple_of(2) {
            0
        } else {
            pitch / 2
        };
        for x in (x0 + off..x1).step_by(pitch as usize) {
            fill(img, x, y, x + 3, y + 3, Rgb([150, 150, 150]));
        }
    }
}

/// 7 セグで数字と '/' を描く（高さ h、幅 h/2）。
fn draw_number(img: &mut RgbImage, s: &str, x: u32, y: u32, h: u32, c: Rgb<u8>) {
    // セグメント: a 上, b 右上, c 右下, d 下, e 左下, f 左上, g 中
    const SEG: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    let w = h / 2;
    let t = (h / 10).max(2);
    let half = h / 2;
    let mut cx = x;
    for ch in s.chars() {
        if ch == '/' {
            for i in 0..h {
                let px = cx + w * (h - i) / h;
                fill(
                    img,
                    px.saturating_sub(t / 2),
                    y + i,
                    px + t / 2,
                    y + i + 1,
                    c,
                );
            }
        } else if let Some(d) = ch.to_digit(10) {
            let m = SEG[d as usize];
            let on = |b: u8| m & (1 << b) != 0;
            if on(0) {
                fill(img, cx, y, cx + w, y + t, c);
            }
            if on(1) {
                fill(img, cx + w - t, y, cx + w, y + half, c);
            }
            if on(2) {
                fill(img, cx + w - t, y + half, cx + w, y + h, c);
            }
            if on(3) {
                fill(img, cx, y + h - t, cx + w, y + h, c);
            }
            if on(4) {
                fill(img, cx, y + half, cx + t, y + h, c);
            }
            if on(5) {
                fill(img, cx, y, cx + t, y + half, c);
            }
            if on(6) {
                fill(img, cx, y + half - t / 2, cx + w, y + half + t / 2, c);
            }
        }
        cx += w + w / 4;
    }
}

fn encode(img: &RgbImage, f: ImageFormat) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, f).expect("encode");
    buf.into_inner()
}

/// 再現できる擬似乱数バイト列（xorshift）。
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

fn write_zip(path: &Path, entries: &[(String, Vec<u8>)]) -> anyhow::Result<()> {
    let mut w = zip::ZipWriter::new(File::create(path)?);
    // JPEG/PNG はもう圧縮済みなので格納のみ
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(name.as_str(), opts)?;
        w.write_all(data)?;
    }
    w.finish()?;
    Ok(())
}

fn write_folder(dir: &Path, pages: u32, hue: usize) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    for p in 1..=pages {
        let img = page(p, pages, hue, W, H);
        fs::write(
            dir.join(format!("p{p:02}.jpg")),
            encode(&img, ImageFormat::Jpeg),
        )?;
    }
    Ok(())
}
