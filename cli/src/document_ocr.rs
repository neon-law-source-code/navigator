//! Local, per-page OCR for Project PDF documents.
//!
//! Source bytes remain in memory/temp storage and are sent to no OCR provider.
//! Poppler renders page images; Tesseract's OSD selects page orientation, and a
//! local projection-profile pass deskews each page before recognition.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use image::{GrayImage, Luma};

#[derive(Debug, Clone, PartialEq, Eq)]
struct OcrText {
    text: String,
    recognized_words: usize,
    confidence: u64,
}

/// OCR a PDF one rendered page at a time, correcting each page's orientation.
pub(crate) fn transcribe_pdf(bytes: &[u8]) -> Result<String> {
    let pages = pdf::page_count(bytes).context("read source PDF page count")?;
    if pages == 0 {
        return Err(anyhow!("source PDF has no pages"));
    }
    let scratch = tempfile::tempdir().context("create private OCR scratch directory")?;
    let input = scratch.path().join("source.pdf");
    std::fs::write(&input, bytes).context("write source PDF to private OCR scratch directory")?;

    let mut transcript = String::new();
    for page_number in 1..=pages {
        let page = scratch.path().join(format!("page-{page_number}"));
        run_command(
            Command::new("pdftoppm")
                .args([
                    "-f",
                    &page_number.to_string(),
                    "-l",
                    &page_number.to_string(),
                ])
                .args(["-singlefile", "-png", "-r", "300"])
                .arg(&input)
                .arg(&page),
            "render PDF page (install Poppler's pdftoppm)",
        )?;
        let image_path = page.with_extension("png");
        let image = image::open(&image_path)
            .with_context(|| format!("open rendered page {page_number}"))?;
        let suggested = orientation(&image_path).unwrap_or(0);
        let mut rotations = vec![suggested, (suggested + 180) % 360];
        let mut best = best_orientation(&image, &rotations, scratch.path())?;
        if best.recognized_words < 3 {
            rotations = vec![0, 90, 180, 270];
            best = best_orientation(&image, &rotations, scratch.path())?;
        }
        transcript.push_str(&format!("## Page {page_number}\n\n"));
        if best.text.trim().is_empty() {
            transcript.push_str("[No text recognized on this page.]\n\n");
        } else {
            transcript.push_str(best.text.trim());
            transcript.push_str("\n\n");
        }
    }
    Ok(transcript)
}

fn orientation(image: &Path) -> Result<u32> {
    let output = Command::new("tesseract")
        .arg(image)
        .arg("stdout")
        .args(["--psm", "0"])
        .output()
        .context(
            "detect page orientation (install Tesseract with the eng and osd language data)",
        )?;
    if !output.status.success() {
        return Err(anyhow!("Tesseract could not detect page orientation"));
    }
    let report = String::from_utf8_lossy(&output.stdout);
    let rotation = report
        .lines()
        .find_map(|line| line.trim().strip_prefix("Rotate:"))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|degrees| matches!(degrees, 0 | 90 | 180 | 270))
        .ok_or_else(|| anyhow!("Tesseract returned no usable page orientation"))?;
    Ok(rotation)
}

fn best_orientation(
    original: &image::DynamicImage,
    rotations: &[u32],
    scratch: &Path,
) -> Result<OcrText> {
    let mut best = OcrText {
        text: String::new(),
        recognized_words: 0,
        confidence: 0,
    };
    for (index, rotation) in rotations.iter().copied().enumerate() {
        let rotated = match rotation {
            0 => original.clone(),
            90 => image::DynamicImage::ImageRgba8(image::imageops::rotate90(original)),
            180 => image::DynamicImage::ImageRgba8(image::imageops::rotate180(original)),
            270 => image::DynamicImage::ImageRgba8(image::imageops::rotate270(original)),
            _ => continue,
        };
        let candidate = scratch.join(format!("orientation-{index}.png"));
        deskew(&rotated)
            .save(&candidate)
            .with_context(|| format!("write deskewed page image for orientation {rotation}"))?;
        let output = Command::new("tesseract")
            .arg(&candidate)
            .arg("stdout")
            .args(["--psm", "3", "tsv"])
            .output()
            .context("OCR page (install Tesseract with the eng language data)")?;
        if !output.status.success() {
            return Err(anyhow!("Tesseract failed while recognizing a page"));
        }
        let recognized = parse_tsv(&String::from_utf8_lossy(&output.stdout));
        if (recognized.recognized_words, recognized.confidence)
            > (best.recognized_words, best.confidence)
        {
            best = recognized;
        }
    }
    Ok(best)
}

fn deskew(image: &image::DynamicImage) -> GrayImage {
    let grayscale = image.to_luma8();
    let correction = estimate_skew(&grayscale);
    if correction.abs() < 0.2 {
        return grayscale;
    }
    rotate_grayscale(&grayscale, correction)
}

/// Estimate the correction angle by maximizing horizontal ink projection.
/// Downsampling bounds the work for phone-camera scans without changing the
/// source image used by OCR.
fn estimate_skew(image: &GrayImage) -> f32 {
    let sample = if image.width() > 500 {
        let height = (u64::from(image.height()) * 500 / u64::from(image.width())) as u32;
        image::imageops::resize(
            image,
            500,
            height.max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    let baseline = projection_score(&sample, 0.0);
    let mut best = (baseline, 0.0f32);
    for half_degrees in -10..=10 {
        let degrees = half_degrees as f32 * 0.5;
        if degrees == 0.0 {
            continue;
        }
        let score = projection_score(&sample, degrees.to_radians());
        if score > best.0 || (score == best.0 && degrees.abs() < best.1.abs()) {
            best = (score, degrees);
        }
    }
    if best.0 as f64 <= baseline as f64 * 1.01 {
        0.0
    } else {
        best.1
    }
}

fn projection_score(image: &GrayImage, radians: f32) -> u64 {
    let width = image.width();
    let height = image.height();
    let center_x = (width as f32 - 1.0) / 2.0;
    let center_y = (height as f32 - 1.0) / 2.0;
    let (sin, cos) = radians.sin_cos();
    let mut rows = vec![0u32; height as usize];
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[0] >= 180 {
            continue;
        }
        let dx = x as f32 - center_x;
        let dy = y as f32 - center_y;
        let rotated_y = (dx * sin + dy * cos + center_y).round() as i32;
        if rotated_y >= 0 {
            if let Some(row) = rows.get_mut(rotated_y as usize) {
                *row += 1;
            }
        }
    }
    rows.into_iter()
        .map(|count| u64::from(count) * u64::from(count))
        .sum()
}

fn rotate_grayscale(image: &GrayImage, degrees: f32) -> GrayImage {
    let (width, height) = image.dimensions();
    let center_x = (width as f32 - 1.0) / 2.0;
    let center_y = (height as f32 - 1.0) / 2.0;
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    GrayImage::from_fn(width, height, |x, y| {
        let dx = x as f32 - center_x;
        let dy = y as f32 - center_y;
        let source_x = (dx * cos + dy * sin + center_x).round() as i32;
        let source_y = (-dx * sin + dy * cos + center_y).round() as i32;
        if source_x >= 0 && source_y >= 0 {
            image
                .get_pixel_checked(source_x as u32, source_y as u32)
                .copied()
                .unwrap_or(Luma([255]))
        } else {
            Luma([255])
        }
    })
}

fn parse_tsv(tsv: &str) -> OcrText {
    let mut lines: BTreeMap<(u32, u32, u32), Vec<(u32, String)>> = BTreeMap::new();
    let mut recognized_words = 0;
    let mut confidence = 0;
    for row in tsv.lines().skip(1) {
        let fields: Vec<_> = row.splitn(12, '\t').collect();
        if fields.len() != 12 || fields[0] != "5" {
            continue;
        }
        let Ok(score) = fields[10].parse::<f32>() else {
            continue;
        };
        if score < 0.0 || fields[11].trim().is_empty() {
            continue;
        }
        let key = (
            fields[2].parse().unwrap_or(0),
            fields[3].parse().unwrap_or(0),
            fields[4].parse().unwrap_or(0),
        );
        let word = fields[11].trim().to_string();
        lines
            .entry(key)
            .or_default()
            .push((fields[5].parse().unwrap_or(0), word));
        recognized_words += usize::from(score >= 40.0);
        confidence += score as u64;
    }
    let text = lines
        .into_values()
        .map(|mut words| {
            words.sort_by_key(|(index, _)| *index);
            words
                .into_iter()
                .map(|(_, word)| word)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    OcrText {
        text,
        recognized_words,
        confidence,
    }
}

fn run_command(command: &mut Command, context: &str) -> Result<()> {
    let output = command.output().with_context(|| context.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("command failed")
            .to_string();
        return Err(anyhow!("{context}: {detail}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{estimate_skew, parse_tsv};
    use image::{GrayImage, Luma};

    #[test]
    fn tesseract_tsv_keeps_page_lines_and_uses_confident_words_for_scoring() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
            5\t1\t1\t1\t1\t2\t10\t10\t20\t10\t91.5\tsecond\n\
            5\t1\t1\t1\t1\t1\t1\t10\t20\t10\t88.0\tFirst\n\
            5\t1\t1\t1\t2\t1\t1\t30\t20\t10\t20.0\tuncertain\n";
        let result = parse_tsv(tsv);
        assert_eq!(result.text, "First second\nuncertain");
        assert_eq!(result.recognized_words, 2);
    }

    #[test]
    fn deskew_estimates_the_inverse_of_slanted_text_lines() {
        let mut image = GrayImage::from_pixel(800, 500, Luma([255]));
        for baseline in [100, 150, 200, 250, 300] {
            for x in 40..760 {
                let y = baseline + (x as f32 * 0.05).round() as u32;
                for thickness in 0..3 {
                    image.put_pixel(x, y + thickness, Luma([0]));
                }
            }
        }
        let correction = estimate_skew(&image);
        assert!((-4.0..-2.0).contains(&correction), "{correction}");
    }
}
