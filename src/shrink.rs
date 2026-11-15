//! Сжатие фото перед отправкой: меньше трафика и быстрее загрузка на медленной связи.
//!
//! Снимок с телефона весит 4–12 МБ, а Telegram всё равно уменьшает фото до 2560 точек по длинной стороне.
//! Поэтому большие JPEG и PNG пережимаются здесь: сторона не больше [`MAX_SIDE`], JPEG качества [`QUALITY`].
//! Поворот из EXIF учитывается (иначе снимок ушёл бы боком), файлы с прозрачностью не трогаются.
//! Если выигрыша нет или файл не получается разобрать, возвращается `None`, и уходит исходный файл.

use std::io::Cursor;

/// Длинная сторона после сжатия.
pub const MAX_SIDE: usize = 2048;
/// Качество JPEG.
pub const QUALITY: u8 = 82;
/// Небольшие файлы не трогаем: выигрыш не стоит потерь качества.
const SKIP_BELOW: usize = 350 * 1024;
/// JPEG больше этого числа точек не разбираем: не хватит памяти слабого компьютера (по три байта на точку).
const MAX_PIXELS: usize = 64_000_000;
/// PNG разбирается в буфер по четыре байта на точку да ещё копируется, поэтому предел ниже. Снимок экрана 4K — 8 млн точек.
const MAX_PIXELS_PNG: usize = 25_000_000;
/// Файлы больше этого размера не читаем целиком.
pub const MAX_INPUT_BYTES: u64 = 40 * 1024 * 1024;

/// Результат сжатия.
pub struct Shrunk {
    pub jpeg: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Картинка в виде строк RGB по три байта на точку.
struct Rgb {
    data: Vec<u8>,
    w: usize,
    h: usize,
}

/// Пробует уменьшить JPEG или PNG. `None` — отправлять исходный файл.
pub fn shrink(bytes: &[u8]) -> Option<Shrunk> {
    let (img, orientation) = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes)?
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        (decode_png(bytes)?, 1)
    } else {
        return None;
    };
    let big = img.w.max(img.h) > MAX_SIDE;
    if bytes.len() < SKIP_BELOW && !big {
        return None;
    }
    let img = if big { downscale(&img, MAX_SIDE) } else { img };
    let img = orient(img, orientation);
    let (w, h) = (img.w, img.h);
    let mut jpeg = Vec::with_capacity(bytes.len() / 4);
    let enc = jpeg_encoder::Encoder::new(&mut jpeg, QUALITY);
    enc.encode(&img.data, u16::try_from(w).ok()?, u16::try_from(h).ok()?, jpeg_encoder::ColorType::Rgb).ok()?;
    // Если не уменьшили размеры и результат не легче исходного, исходный файл лучше.
    if !big && jpeg.len() >= bytes.len() {
        return None;
    }
    Some(Shrunk { jpeg, width: w, height: h })
}

fn decode_jpeg(bytes: &[u8]) -> Option<(Rgb, u8)> {
    use jpeg_decoder::PixelFormat;
    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if w == 0 || h == 0 || w * h > MAX_PIXELS {
        return None;
    }
    let orientation = dec.exif_data().map_or(1, exif_orientation);
    let pixels = dec.decode().ok()?;
    let data = match info.pixel_format {
        PixelFormat::RGB24 if pixels.len() >= w * h * 3 => pixels,
        PixelFormat::L8 if pixels.len() >= w * h => pixels.iter().flat_map(|g| [*g, *g, *g]).collect(),
        _ => return None, // CMYK, 16 бит: пусть уходит как есть
    };
    Some((Rgb { data, w, h }, orientation))
}

fn decode_png(bytes: &[u8]) -> Option<Rgb> {
    use png::{ColorType, Transformations};
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    if w == 0 || h == 0 || w * h > MAX_PIXELS_PNG {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..frame.buffer_size()];
    let data: Vec<u8> = match frame.color_type {
        ColorType::Rgb if px.len() >= w * h * 3 => px[..w * h * 3].to_vec(),
        ColorType::Grayscale if px.len() >= w * h => px[..w * h].iter().flat_map(|g| [*g, *g, *g]).collect(),
        // С прозрачностью картинку не трогаем: в JPEG её не сохранить.
        ColorType::Rgba if px.len() >= w * h * 4 => {
            if px[..w * h * 4].as_chunks::<4>().0.iter().any(|p| p[3] != 255) {
                return None;
            }
            px[..w * h * 4].as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
        }
        ColorType::GrayscaleAlpha if px.len() >= w * h * 2 => {
            if px[..w * h * 2].as_chunks::<2>().0.iter().any(|p| p[1] != 255) {
                return None;
            }
            px[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0]]).collect()
        }
        _ => return None,
    };
    Some(Rgb { data, w, h })
}

/// Поворот снимка из EXIF (1…8; 1 — без поворота). Любая ошибка разбора — без поворота.
fn exif_orientation(exif: &[u8]) -> u8 {
    let t = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let little = match t.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return 1,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let find = || -> Option<u8> {
        if u16_at(2)? != 42 {
            return None;
        }
        let ifd = u32_at(4)? as usize;
        let count = usize::from(u16_at(ifd)?).min(256);
        (0..count)
            .find_map(|i| {
                let e = ifd + 2 + i * 12;
                (u16_at(e)? == 0x0112).then(|| u16_at(e + 8)).flatten()
            })
            .filter(|v| (1..=8).contains(v))
            .map(|v| v as u8)
    };
    find().unwrap_or(1)
}

/// Применяет поворот EXIF к уже уменьшенной картинке.
fn orient(img: Rgb, o: u8) -> Rgb {
    if o <= 1 || o > 8 {
        return img;
    }
    let (w, h) = (img.w, img.h);
    let swap = o >= 5;
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        for x in 0..dw {
            // Откуда в исходной картинке берётся точка (x, y) результата.
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x),
            };
            let (s, d) = ((sy * w + sx) * 3, (y * dw + x) * 3);
            out[d..d + 3].copy_from_slice(&img.data[s..s + 3]);
        }
    }
    Rgb { data: out, w: dw, h: dh }
}

/// Уменьшает так, чтобы длинная сторона стала `max_side`; пропорции сохраняются, точки усредняются по площади.
fn downscale(img: &Rgb, max_side: usize) -> Rgb {
    let long = img.w.max(img.h);
    let dw = ((img.w * max_side + long / 2) / long).max(1);
    let dh = ((img.h * max_side + long / 2) / long).max(1);
    let (sx, sy) = (img.w as f64 / dw as f64, img.h as f64 / dh as f64);
    let mut out = vec![0u8; dw * dh * 3];
    let mut acc = vec![0f32; img.w * 3];
    for dy in 0..dh {
        // Строки исходника, попавшие в эту строку результата, с весами по доле перекрытия.
        let (y0, y1) = (dy as f64 * sy, (((dy + 1) as f64) * sy).min(img.h as f64));
        acc.fill(0.0);
        let mut i = y0.floor() as usize;
        while (i as f64) < y1 && i < img.h {
            let weight = ((((i + 1) as f64).min(y1) - (i as f64).max(y0)) / sy) as f32;
            if weight > 0.0 {
                let row = &img.data[i * img.w * 3..(i + 1) * img.w * 3];
                for (a, p) in acc.iter_mut().zip(row) {
                    *a += f32::from(*p) * weight;
                }
            }
            i += 1;
        }
        for dx in 0..dw {
            let (x0, x1) = (dx as f64 * sx, (((dx + 1) as f64) * sx).min(img.w as f64));
            let mut sum = [0f32; 3];
            let mut j = x0.floor() as usize;
            while (j as f64) < x1 && j < img.w {
                let weight = ((((j + 1) as f64).min(x1) - (j as f64).max(x0)) / sx) as f32;
                if weight > 0.0 {
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += acc[j * 3 + c] * weight;
                    }
                }
                j += 1;
            }
            for (c, s) in sum.iter().enumerate() {
                out[(dy * dw + dx) * 3 + c] = s.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Rgb { data: out, w: dw, h: dh }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JPEG из готовых RGB-точек; `exif` — необязательный блок EXIF (без заголовка «Exif»).
    fn jpeg_of(data: &[u8], w: usize, h: usize, exif: Option<&[u8]>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = jpeg_encoder::Encoder::new(&mut out, 95);
        if let Some(e) = exif {
            enc.add_exif_metadata(e).unwrap();
        }
        enc.encode(data, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).unwrap();
        out
    }

    fn png_of(data: &[u8], w: usize, h: usize, color: png::ColorType) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    /// Плавный градиент с шумом: сжимается плохо, как настоящий снимок.
    fn photo(w: usize, h: usize) -> Vec<u8> {
        let mut seed = 12345u32;
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (seed >> 24) as i32 % 24;
                v.push(((x * 255 / w) as i32 + n).clamp(0, 255) as u8);
                v.push(((y * 255 / h) as i32 + n).clamp(0, 255) as u8);
                v.push((((x + y) * 255 / (w + h)) as i32 + n).clamp(0, 255) as u8);
            }
        }
        v
    }

    /// Точка (x, y) декодированного JPEG.
    fn pixel(jpeg: &[u8], x: usize, y: usize) -> [u8; 3] {
        let mut d = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
        let px = d.decode().unwrap();
        let info = d.info().unwrap();
        let i = (y * usize::from(info.width) + x) * 3;
        [px[i], px[i + 1], px[i + 2]]
    }

    fn dims(jpeg: &[u8]) -> (usize, usize) {
        let mut d = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
        d.read_info().unwrap();
        let i = d.info().unwrap();
        (usize::from(i.width), usize::from(i.height))
    }

    /// Блок EXIF с одним полем «Orientation» (порядок байт Intel).
    fn exif_with(orientation: u16) -> Vec<u8> {
        let mut e = b"II*\0".to_vec();
        e.extend_from_slice(&8u32.to_le_bytes()); // IFD сразу за заголовком
        e.extend_from_slice(&1u16.to_le_bytes()); // одно поле
        e.extend_from_slice(&0x0112u16.to_le_bytes());
        e.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        e.extend_from_slice(&1u32.to_le_bytes());
        e.extend_from_slice(&orientation.to_le_bytes());
        e.extend_from_slice(&[0, 0]);
        e.extend_from_slice(&0u32.to_le_bytes()); // следующего IFD нет
        e
    }

    #[test]
    fn a_big_photo_shrinks_to_the_side_limit_and_gets_much_lighter() {
        let src = jpeg_of(&photo(3200, 2400), 3200, 2400, None);
        let out = shrink(&src).expect("большой снимок сжимается");
        assert_eq!((out.width, out.height), (2048, 1536));
        assert_eq!(dims(&out.jpeg), (2048, 1536));
        assert!(out.jpeg.len() < src.len() / 2, "{} -> {}", src.len(), out.jpeg.len());
        // Цвета сохраняются: левый верхний угол градиента тёмный, правый нижний светлый.
        let (a, b) = (pixel(&out.jpeg, 4, 4), pixel(&out.jpeg, 2040, 1530));
        assert!(a[0] < 40 && b[0] > 215, "{a:?} {b:?}");
        // Вертикальный снимок ограничивается по высоте.
        let tall = jpeg_of(&photo(1500, 3000), 1500, 3000, None);
        let out = shrink(&tall).unwrap();
        assert_eq!((out.width, out.height), (1024, 2048));
    }

    #[test]
    fn small_files_and_non_images_are_left_alone() {
        let small = jpeg_of(&photo(200, 150), 200, 150, None);
        assert!(small.len() < SKIP_BELOW);
        assert!(shrink(&small).is_none());
        assert!(shrink(b"").is_none());
        assert!(shrink(b"just some text, not an image").is_none());
        assert!(shrink(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).is_none(), "обрезанный JPEG");
        assert!(shrink(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]).is_none(), "обрезанный PNG");
        // Обрезанный на середине настоящий снимок не роняет программу и не отправляется испорченным.
        let src = jpeg_of(&photo(2600, 1800), 2600, 1800, None);
        assert!(shrink(&src[..src.len() / 2]).is_none());
    }

    #[test]
    fn exif_rotation_is_applied_so_portrait_photos_do_not_go_sideways() {
        // Слева красная половина, справа синяя; снимок помечен «повернуть на 90° по часовой».
        let (w, h) = (2600usize, 1000usize);
        let mut px = Vec::with_capacity(w * h * 3);
        for _y in 0..h {
            for x in 0..w {
                px.extend_from_slice(if x < w / 2 { &[255, 0, 0] } else { &[0, 0, 255] });
            }
        }
        let src = jpeg_of(&px, w, h, Some(&exif_with(6)));
        let out = shrink(&src).expect("снимок с поворотом");
        // 2600×1000 → 2048×788, поворот на 90° меняет стороны местами.
        assert_eq!((out.width, out.height), (788, 2048));
        assert_eq!(dims(&out.jpeg), (788, 2048));
        // Левая (красная) половина стала верхней, правая (синяя) нижней.
        let (top, bottom) = (pixel(&out.jpeg, 400, 100), pixel(&out.jpeg, 400, 1950));
        assert!(top[0] > 200 && top[2] < 60, "{top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "{bottom:?}");
        // Без метки поворота снимок остаётся альбомным.
        let plain = jpeg_of(&px, w, h, None);
        assert_eq!(shrink(&plain).map(|o| (o.width, o.height)), Some((2048, 788)));
    }

    #[test]
    fn every_orientation_maps_the_corners_as_exif_says() {
        // 3×2: верхний ряд A B C, нижний D E F (каждая точка — своё значение).
        let base = Rgb { data: (0u8..6).flat_map(|v| [v, v, v]).collect(), w: 3, h: 2 };
        let cases: [(u8, (usize, usize), [u8; 6]); 8] = [
            (1, (3, 2), [0, 1, 2, 3, 4, 5]),
            (2, (3, 2), [2, 1, 0, 5, 4, 3]),
            (3, (3, 2), [5, 4, 3, 2, 1, 0]),
            (4, (3, 2), [3, 4, 5, 0, 1, 2]),
            (5, (2, 3), [0, 3, 1, 4, 2, 5]),
            (6, (2, 3), [3, 0, 4, 1, 5, 2]),
            (7, (2, 3), [5, 2, 4, 1, 3, 0]),
            (8, (2, 3), [2, 5, 1, 4, 0, 3]),
        ];
        for (o, (w, h), want) in cases {
            let img = Rgb { data: base.data.clone(), w: base.w, h: base.h };
            let out = orient(img, o);
            assert_eq!((out.w, out.h), (w, h), "orientation {o}");
            let got: Vec<u8> = out.data.chunks(3).map(|p| p[0]).collect();
            assert_eq!(got, want, "orientation {o}");
        }
    }

    #[test]
    fn exif_parsing_is_defensive() {
        assert_eq!(exif_orientation(&exif_with(6)), 6);
        let mut with_header = b"Exif\0\0".to_vec();
        with_header.extend_from_slice(&exif_with(8));
        assert_eq!(exif_orientation(&with_header), 8);
        // Порядок байт Motorola.
        let mut be = b"MM\0*".to_vec();
        be.extend_from_slice(&8u32.to_be_bytes());
        be.extend_from_slice(&1u16.to_be_bytes());
        be.extend_from_slice(&0x0112u16.to_be_bytes());
        be.extend_from_slice(&3u16.to_be_bytes());
        be.extend_from_slice(&1u32.to_be_bytes());
        be.extend_from_slice(&3u16.to_be_bytes());
        be.extend_from_slice(&[0, 0]);
        assert_eq!(exif_orientation(&be), 3);
        for bad in [&b""[..], b"II", b"XX*\0", b"II*\0\xff\xff\xff\xff", &exif_with(0), &exif_with(9), &exif_with(6)[..10]] {
            assert_eq!(exif_orientation(bad), 1, "{bad:?}");
        }
        // Слишком много полей в каталоге: не читаем за пределы данных.
        let mut huge = b"II*\0".to_vec();
        huge.extend_from_slice(&8u32.to_le_bytes());
        huge.extend_from_slice(&65_535u16.to_le_bytes());
        assert_eq!(exif_orientation(&huge), 1);
    }

    #[test]
    fn png_screenshots_become_jpeg_but_transparent_ones_stay() {
        let (w, h) = (2400, 1500);
        let rgb = photo(w, h);
        let png = png_of(&rgb, w, h, png::ColorType::Rgb);
        assert!(png.len() > SKIP_BELOW);
        let out = shrink(&png).expect("PNG сжимается");
        assert_eq!((out.width, out.height), (2048, 1280));
        assert!(out.jpeg.len() < png.len(), "{} -> {}", png.len(), out.jpeg.len());
        assert_eq!(dims(&out.jpeg), (2048, 1280));
        // С прозрачностью — как есть.
        let mut rgba: Vec<u8> = rgb.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        rgba[3] = 0;
        assert!(shrink(&png_of(&rgba, w, h, png::ColorType::Rgba)).is_none());
        // Без прозрачности RGBA и серый тоже годятся.
        rgba[3] = 255;
        assert!(shrink(&png_of(&rgba, w, h, png::ColorType::Rgba)).is_some());
        let gray: Vec<u8> = rgb.chunks(3).map(|p| p[0]).collect();
        let out = shrink(&png_of(&gray, w, h, png::ColorType::Grayscale)).expect("серый PNG");
        assert_eq!(out.width, 2048);
    }

    #[test]
    fn downscale_averages_by_area_and_keeps_colors() {
        // Однотонная картинка остаётся однотонной при любом коэффициенте.
        let flat = Rgb { data: [200, 100, 50].repeat(100 * 70), w: 100, h: 70 };
        let out = downscale(&flat, 33);
        assert_eq!((out.w, out.h), (33, 23));
        assert!(out.data.chunks(3).all(|p| p == [200, 100, 50]));
        // Две соседние точки 0 и 255 при уменьшении вдвое дают среднее.
        let pair = Rgb { data: [0, 0, 0, 255, 255, 255].to_vec(), w: 2, h: 1 };
        let out = downscale(&pair, 1);
        assert_eq!((out.w, out.h), (1, 1));
        assert_eq!(out.data, [128, 128, 128]);
        // Крайние размеры не вызывают паники.
        let line = Rgb { data: vec![9; 3 * 5000], w: 5000, h: 1 };
        let out = downscale(&line, 100);
        assert_eq!((out.w, out.h), (100, 1));
    }

    #[test]
    fn grayscale_jpeg_is_shrunk_and_cmyk_is_left_alone() {
        let gray: Vec<u8> = photo(2300, 1700).chunks(3).map(|p| p[0]).collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 95).encode(&gray, 2300, 1700, jpeg_encoder::ColorType::Luma).unwrap();
        let s = shrink(&out).expect("серый JPEG");
        assert_eq!((s.width, s.height), (2048, 1514));
        // CMYK не разбираем: он уходит как есть.
        let cmyk: Vec<u8> = (0..2300 * 1700 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 90).encode(&cmyk, 2300, 1700, jpeg_encoder::ColorType::Cmyk).unwrap();
        assert!(shrink(&out).is_none());
    }

    /// Повреждённые файлы встречаются в жизни (оборванная загрузка, битая карта памяти). В настоящей сборке
    /// паника завершает программу, поэтому разбор картинок не должен паниковать ни на каких данных.
    /// Больше кругов: `TGL_FUZZ_SHRINK=3000 cargo test damaged_pictures`.
    #[test]
    fn damaged_pictures_never_panic() {
        let rounds: usize = std::env::var("TGL_FUZZ_SHRINK").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
        let (w, h) = (2100usize, 16usize); // шире предела: проходят и уменьшение, и сжатие
        let rgb = photo(w, h);
        let originals = [("jpeg", jpeg_of(&rgb, w, h, Some(&exif_with(6)))), ("png", png_of(&rgb, w, h, png::ColorType::Rgb))];
        let mut seed = 0x9E37_79B9u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as usize
        };
        let mut failures = Vec::new();
        for (name, original) in &originals {
            for round in 0..rounds {
                let mut data = original.clone();
                match next() % 4 {
                    0 => {
                        for _ in 0..1 + next() % 4 {
                            let i = next() % data.len().min(700);
                            data[i] = next() as u8;
                        }
                    }
                    1 => {
                        for _ in 0..1 + next() % 8 {
                            let i = next() % data.len();
                            data[i] ^= 1 << (next() % 8);
                        }
                    }
                    2 => data.truncate(next() % data.len()),
                    _ => {
                        let at = next() % data.len();
                        for _ in 0..1 + next() % 16 {
                            data.insert(at, next() as u8);
                        }
                    }
                }
                if std::panic::catch_unwind(|| shrink(&data)).is_err() {
                    failures.push(format!("{name} #{round}"));
                }
            }
        }
        assert!(failures.is_empty(), "паника на повреждённых файлах: {failures:?}");
    }
}
