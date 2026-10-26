"""Generate original synthetic media. No private or copyrighted books are used."""
import os
from pathlib import Path
import subprocess
import sys
import zipfile

root = Path(sys.argv[1])
root.mkdir(parents=True, exist_ok=True)
book = root / "Audiobooks" / "The Quiet Hours"
book.mkdir(parents=True, exist_ok=True)
for number, frequency in [(1, 220), (2, 330)]:
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                    f"sine=frequency={frequency}:duration=24", "-af", "volume=0.03", "-c:a", "libmp3lame",
                    "-metadata", "album=The Quiet Hours", "-metadata", "artist=Ellis Rowan",
                    "-metadata", f"track={number}", str(book / f"Part {number}.mp3")], check=True)
(book / "metadata.opf").write_text("<metadata><title>The Quiet Hours</title><creator>Ellis Rowan</creator><description>An original synthetic audiobook for testing a quieter kind of listening.</description><date>2026</date></metadata>")

chaptered = root / "Audiobooks" / "A Field Guide to Elsewhere"
chaptered.mkdir(parents=True, exist_ok=True)
chapters = Path("/tmp/kapitel-chapters.txt")
chapters.write_text(";FFMETADATA1\ntitle=A Field Guide to Elsewhere\nartist=June Wilder\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=12000\ntitle=Leaving\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=12000\nEND=24000\ntitle=Arriving\n")
subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=24",
                "-i", str(chapters), "-map_metadata", "1", "-map_chapters", "1", "-af", "volume=0.03", "-c:a", "aac",
                str(chaptered / "book.m4b")], check=True)

for number, (title, author) in enumerate([
    ("The Art of Noticing", "Wren Adler"), ("Letters from the Garden", "Ada Finch"),
    ("A Map of Small Things", "Noah Wells"), ("The Long Way Home", "Mira Lane"),
    ("On Ordinary Days", "Robin Vale"), ("The Shape of Silence", "Kit Moss"),
]):
    folder = root / "Ebooks" / title
    folder.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(folder / "book.epub", "w") as archive:
        archive.writestr("mimetype", "application/epub+zip")
        archive.writestr("META-INF/container.xml", '<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>')
        archive.writestr("OEBPS/content.opf", f'<package><metadata><title>{title}</title><creator>{author}</creator><date>2026</date></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>')
        archive.writestr("OEBPS/chapter.xhtml", f'<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>{title}</h1><p>Original synthetic test fixture {number}.</p></body></html>')

for directory, _, files in os.walk(root):
    os.chmod(directory, 0o755)
    for name in files:
        os.chmod(Path(directory) / name, 0o644)
print("Generated 2 synthetic audiobooks and 6 ebooks")
