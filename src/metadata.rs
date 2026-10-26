use anyhow::{Context, bail, ensure};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::{Accessor, ItemKey},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    pub author: String,
    pub narrator: String,
    pub publication_year: String,
    pub language: String,
    pub description: String,
    pub duration_seconds: f64,
    pub track_number: u32,
    pub disc_number: u32,
    pub chapters: Vec<Chapter>,
    pub cover: Option<Vec<u8>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    pub start_seconds: f64,
}

impl Metadata {
    pub fn overlay(&mut self, source: &Self) {
        for (target, value) in [
            (&mut self.title, &source.title),
            (&mut self.author, &source.author),
            (&mut self.narrator, &source.narrator),
            (&mut self.publication_year, &source.publication_year),
            (&mut self.language, &source.language),
            (&mut self.description, &source.description),
        ] {
            if !value.is_empty() {
                *target = value.clone();
            }
        }
        if source.cover.is_some() {
            self.cover = source.cover.clone();
        }
    }
    fn bound(&mut self) -> anyhow::Result<()> {
        for field in [
            &mut self.title,
            &mut self.author,
            &mut self.narrator,
            &mut self.publication_year,
            &mut self.language,
        ] {
            *field = field
                .chars()
                .filter(|c| !c.is_control())
                .take(500)
                .collect();
        }
        self.description = self.description.chars().take(8000).collect();
        ensure!(
            self.duration_seconds.is_finite() && self.duration_seconds >= 0.0,
            "Invalid duration"
        );
        ensure!(self.chapters.len() <= 5000, "Too many chapter markers");
        self.chapters.retain(|chapter| {
            chapter.start_seconds.is_finite()
                && chapter.start_seconds >= 0.0
                && chapter.start_seconds < self.duration_seconds
        });
        self.chapters
            .sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
        self.chapters
            .dedup_by(|a, b| a.start_seconds == b.start_seconds);
        for chapter in &mut self.chapters {
            chapter.title = chapter.title.chars().take(500).collect();
        }
        Ok(())
    }
}

pub fn inspect(mut file: File, extension: &str) -> anyhow::Result<Metadata> {
    lofty::config::apply_global_options(
        lofty::config::GlobalOptions::new().allocation_limit(16 * 1024 * 1024),
    );
    let mut metadata = Metadata::default();
    match extension {
        "mp3" | "m4a" | "m4b" => {
            let audio = lofty::read_from(&mut file).context("Cannot read audio metadata")?;
            let expected = if extension == "mp3" {
                lofty::file::FileType::Mpeg
            } else {
                lofty::file::FileType::Mp4
            };
            ensure!(
                audio.file_type() == expected,
                "File contents do not match the audio extension"
            );
            metadata.duration_seconds = audio.properties().duration().as_secs_f64();
            if let Some(tag) = audio.primary_tag().or_else(|| audio.first_tag()) {
                metadata.title = tag
                    .album()
                    .or_else(|| tag.title())
                    .unwrap_or_default()
                    .into_owned();
                metadata.author = tag
                    .get_string(ItemKey::AlbumArtist)
                    .or_else(|| tag.get_string(ItemKey::TrackArtist))
                    .unwrap_or_default()
                    .into();
                metadata.narrator = tag.get_string(ItemKey::Composer).unwrap_or_default().into();
                metadata.publication_year = tag
                    .get_string(ItemKey::RecordingDate)
                    .unwrap_or_default()
                    .chars()
                    .take(4)
                    .collect();
                metadata.language = tag.get_string(ItemKey::Language).unwrap_or_default().into();
                metadata.description = tag
                    .get_string(ItemKey::Description)
                    .unwrap_or_default()
                    .into();
                metadata.track_number = tag.track().unwrap_or(0);
                metadata.disc_number = tag.disk().unwrap_or(0);
                if let Some(picture) = tag.pictures().first() {
                    metadata.cover = Some(thumbnail(picture.data())?);
                }
            }
            file.seek(SeekFrom::Start(0))?;
            if extension == "mp3" {
                let mp3 = lofty::mpeg::MpegFile::read_from(
                    &mut file,
                    lofty::config::ParseOptions::new(),
                )?;
                if let Some(tag) = mp3.id3v2() {
                    for frame in tag {
                        if let lofty::id3::v2::Frame::Chapter(chapter) = frame {
                            metadata.chapters.push(Chapter {
                                title: chapter.title().map(str::to_owned).unwrap_or_else(|| {
                                    format!("Chapter {}", metadata.chapters.len() + 1)
                                }),
                                start_seconds: chapter.times.start as f64 / 1000.0,
                            });
                        }
                    }
                }
            } else {
                let mp4 = lofty::mp4::Mp4File::read_from(
                    &mut file,
                    lofty::config::ParseOptions::new().read_tags(false),
                )?;
                ensure!(
                    mp4.properties().codec() == Some(lofty::mp4::Mp4Codec::AAC),
                    "Only AAC audio in M4A/M4B is supported in v0.1"
                );
                file.seek(SeekFrom::Start(0))?;
                let tag = mp4ameta::Tag::read_from(&mut file)?;
                ensure!(
                    tag.audio_info().channel_config.is_some(),
                    "Missing MP4 audio configuration"
                );
                metadata.chapters = tag
                    .chapters()
                    .iter()
                    .map(|chapter| Chapter {
                        title: chapter.title.clone(),
                        start_seconds: chapter.start.as_secs_f64(),
                    })
                    .collect();
            }
            ensure!(metadata.duration_seconds > 0.0, "Audio duration is missing");
        }
        "epub" => {
            metadata = epub(file)?;
        }
        "opf" => {
            metadata = opf(&read_limited(file, MAX_METADATA_BYTES)?)?;
        }
        "jpg" | "jpeg" | "png" | "webp" => {
            metadata.cover = Some(thumbnail(&read_limited(file, 8 * 1024 * 1024)?)?);
        }
        "pdf" => {
            let mut magic = [0; 5];
            file.read_exact(&mut magic)?;
            ensure!(&magic == b"%PDF-", "Invalid PDF signature");
        }
        "azw3" => {
            file.seek(SeekFrom::Start(60))?;
            let mut magic = [0; 8];
            file.read_exact(&mut magic)?;
            ensure!(&magic == b"BOOKMOBI", "Invalid AZW3 signature");
        }
        _ => bail!("Unsupported file format"),
    }
    metadata.bound()?;
    Ok(metadata)
}

fn read_limited(reader: impl Read, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Metadata exceeds the size limit"
    );
    Ok(bytes)
}

pub fn opf(bytes: &[u8]) -> anyhow::Result<Metadata> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut current: Option<(String, usize)> = None;
    let mut text_value = String::new();
    let mut metadata = Metadata::default();
    let mut depth = 0;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::DocType(_) => bail!("XML document types are not allowed"),
            quick_xml::events::Event::Start(element) => {
                depth += 1;
                ensure!(depth < 64, "XML nesting is too deep");
                let name = element.local_name().as_ref().to_string();
                if current.is_none()
                    && ["title", "creator", "date", "language", "description"]
                        .contains(&name.as_str())
                {
                    current = Some((name, depth));
                    text_value.clear();
                }
            }
            quick_xml::events::Event::End(_) => {
                if current
                    .as_ref()
                    .is_some_and(|(_, started)| *started == depth)
                {
                    let (name, _) = current.take().unwrap();
                    let value = text_value.trim().to_string();
                    match name.as_str() {
                        "title" => metadata.title = value,
                        "creator" => {
                            if !metadata.author.is_empty() {
                                metadata.author.push_str(", ");
                            }
                            metadata.author.push_str(&value);
                        }
                        "date" => {
                            metadata.publication_year =
                                value.split('-').next().unwrap_or_default().into()
                        }
                        "language" => metadata.language = value,
                        "description" => metadata.description = value,
                        _ => {}
                    }
                }
                depth -= 1;
            }
            quick_xml::events::Event::Text(text) if current.is_some() => {
                text_value.push_str(&text);
            }
            quick_xml::events::Event::CData(text) if current.is_some() => {
                text_value.push_str(&text);
            }
            quick_xml::events::Event::GeneralRef(reference) if current.is_some() => {
                let entity = format!("&{};", &*reference);
                text_value.push_str(&quick_xml::escape::unescape(&entity)?);
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    metadata.bound()?;
    Ok(metadata)
}

fn epub(file: File) -> anyhow::Result<Metadata> {
    let mut archive = zip::ZipArchive::new(file)?;
    ensure!(archive.len() <= 4096, "EPUB contains too many entries");
    let container = read_limited(
        archive.by_name("META-INF/container.xml")?,
        MAX_METADATA_BYTES,
    )?;
    let mut reader = quick_xml::Reader::from_reader(container.as_slice());
    let mut package_path = None;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::DocType(_) => bail!("XML document types are not allowed"),
            quick_xml::events::Event::Empty(element) | quick_xml::events::Event::Start(element)
                if element.local_name().as_ref() == "rootfile" =>
            {
                for attr in element.attributes() {
                    let attr = attr?;
                    if attr.key.as_ref() == "full-path" {
                        package_path = Some(
                            attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                .to_string(),
                        );
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let package_path = package_path.context("EPUB has no package metadata")?;
    ensure!(
        Path::new(&package_path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))),
        "Invalid EPUB package path"
    );
    let package = read_limited(archive.by_name(&package_path)?, MAX_METADATA_BYTES)?;
    let mut metadata = opf(&package)?;
    let mut reader = quick_xml::Reader::from_reader(package.as_slice());
    let mut cover_path = None;
    let mut cover_id = String::new();
    let mut items = Vec::new();
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(element) | quick_xml::events::Event::Start(element) => {
                let attributes = element
                    .attributes()
                    .map(|attr| {
                        let attr = attr?;
                        Ok((
                            attr.key.as_ref().to_string(),
                            attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                .to_string(),
                        ))
                    })
                    .collect::<anyhow::Result<std::collections::HashMap<_, _>>>()?;
                if element.local_name().as_ref() == "meta"
                    && attributes.get("name").is_some_and(|name| name == "cover")
                {
                    cover_id = attributes.get("content").cloned().unwrap_or_default();
                }
                if element.local_name().as_ref() == "item" {
                    items.push(attributes);
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    for item in items {
        if item
            .get("properties")
            .is_some_and(|value| value.split_whitespace().any(|part| part == "cover-image"))
            || (!cover_id.is_empty() && item.get("id") == Some(&cover_id))
        {
            cover_path = item.get("href").cloned();
            break;
        }
    }
    if let Some(cover_path) = cover_path {
        ensure!(
            Path::new(&cover_path)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "Invalid EPUB cover path"
        );
        let path = Path::new(&package_path)
            .parent()
            .unwrap_or(Path::new(""))
            .join(cover_path);
        metadata.cover = Some(thumbnail(&read_limited(
            archive.by_name(&path.to_string_lossy())?,
            8 * 1024 * 1024,
        )?)?);
    }
    Ok(metadata)
}

pub fn thumbnail(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    ensure!(bytes.len() <= 8 * 1024 * 1024, "Cover exceeds 8 MiB");
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.thumbnail(360, 480).to_rgb8();
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 80).encode_image(&image)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opf_rejects_entities_and_limits_nesting() {
        assert!(
            opf(b"<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><title>&x;</title>")
                .is_err()
        );
        let nested = format!("{}{}", "<x>".repeat(70), "</x>".repeat(70));
        assert!(opf(nested.as_bytes()).is_err());
        let data=opf(b"<metadata><dc:title xmlns:dc='x'>A book</dc:title><creator>Author</creator><date>2024-01-02</date></metadata>").unwrap();
        assert_eq!(data.title, "A book");
        assert_eq!(data.publication_year, "2024");
        let entities = opf(b"<metadata><title>A &amp; B &#x26; C</title><description>A <b>bold</b> word</description></metadata>").unwrap();
        assert_eq!(entities.title, "A & B & C");
        assert_eq!(entities.description, "A bold word");
    }
}
