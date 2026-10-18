use std::io::Cursor;
use std::path::Path;

use chrono::Local;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader};

use crate::{Error, Result, bail};

const MAX_SIDE: u32 = 1600;

/// 上傳的圖片：依 EXIF 轉正、縮到長邊 1600px、存成 JPEG。回傳相對於圖片資料夾的路徑。
pub fn save_upload(media: &Path, kind: &str, bytes: &[u8]) -> Result<String> {
    if !matches!(kind, "products" | "purchases") {
        return bail("未知的圖片類型");
    }
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_decoder()
        .map_err(|_| Error("無法辨識的圖片格式，請使用 JPG、PNG 或 WEBP".into()))?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, FilterType::Lanczos3);
    }
    let now = Local::now();
    let rel = format!("{kind}/{}/{}.jpg", now.format("%Y/%m"), now.format("%Y%m%d%H%M%S%f"));
    let path = media.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap_or(media))?;
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 85).encode_image(&img.to_rgb8())?;
    std::fs::write(&path, buf)?;
    Ok(rel)
}

pub fn thumbnail(path: &Path, size: u32) -> Result<Vec<u8>> {
    let img = image::open(path)?.thumbnail(size, size).to_rgb8();
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 80).encode_image(&img)?;
    Ok(buf)
}
