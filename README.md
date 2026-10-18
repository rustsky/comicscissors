# comic-kindle

Detects comic panels, crops them, and repacks them onto pages sized for a specific Kindle,
so every panel is readable without zooming.

![A comic page with five detected panels, repacked onto three Kindle Paperwhite pages](docs/demo.jpg)

<sub>Sample page: *America's Best Comics* #18 (1946), "The Black Terror", art by Ed Moritz.
Public domain, via [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:AmericasBestComics1804.jpg).</sub>

## Build

```sh
cargo build --release
# binary: target/release/comic-magic
```

PDF input needs `mutool` (MuPDF) or `pdftoppm` (Poppler) on PATH (`brew install mupdf`).
CBR/CB7 input uses `bsdtar` (built into macOS), `unar` or `7z`.
The dictionary text layer needs Tesseract (`brew install tesseract`). English is included; for
other languages, run `brew install tesseract-lang`.

## Calibre plugin

Each [release](https://github.com/rustsky/comic_magic/releases) has one archive per platform
(`linux-x86_64`, `macos-universal`, `windows-x86_64`) that holds the `comic-magic` CLI and
`comic-magic-calibre-plugin.zip`. The plugin includes the same binary.

Install it with *Preferences → Plugins → Load plugin from file*, then add the **Repack for Kindle**
button to a toolbar. Select comics (CBZ, CBR, CB7 or PDF) and click it. The result is added to
the same book as a new format (e.g. a CBZ book gets a PDF), so it can be sent to the Kindle with
*Send to device*. An original file is never replaced: when the book already has an original file
of the output type (a PDF comic repacked to PDF), the result is saved as the data file
`data/Title - Author_repanel.pdf` instead, shown under *Data files* in Book details. Repacking
again replaces only the earlier result.
Use the button's menu → *Settings* to choose the Kindle, the output format, the language, manga mode and extra CLI flags.

PDFs are rendered with the Poppler tools bundled in Calibre, and CBR/CB7 files are unpacked by
Calibre, so nothing else needs to be installed. The text layer still needs Tesseract. Without it,
the plugin converts without the text layer.

Releases: bump `version` in `Cargo.toml`, commit, then `git tag v<version> && git push --tags`.
CI builds the three archives and publishes the release. To build an archive locally:

```sh
cargo build --release
python3 scripts/package.py --binary target/release/comic-magic --platform macos-arm64   # -> dist/
```

## Use

```sh
comic-magic mycomic.cbz                       # asks which Kindle you have and the comic's language
comic-magic mycomic.pdf -d paperwhite-5       # -> mycomic_repanel.pdf
comic-magic pages/ -d kindle-11 -f cbz        # folder of images -> CBZ
comic-magic manga.cbz -d oasis --rtl --print-height 7.2 --lang ja
comic-magic bd.cbz -d paperwhite-5 --lang fr       # French text layer
comic-magic --list-devices
comic-magic in.cbz --width 1072 --height 1448 --ppi 300   # custom screen
```

Copy the output PDF to the Kindle's `documents` folder over USB. Each page is exactly the
size of the screen.

## How it works

1. **Input**: PDF pages are rendered at `--pdf-dpi`. Archives are extracted, and images are
   sorted in natural order (`p2` comes before `p10`).
2. **Panel detection**: the paper colour is the median of the page edge. Everything that differs
   from it by more than `--threshold` counts as ink. Paper reachable from the page edge, plus thin,
   long strips of paper, form the gutters. Everything else becomes a solid blob, with each panel's
   interior filled in. Each blob is one panel candidate. When a speech balloon crosses a gutter and
   joins two panels, the blob is cut along lines that are mostly empty (`--split-coverage`). Panels
   are then put in reading order: rows top to bottom, then left to right, or right to left with
   `--rtl`. Small regions such as page numbers are dropped.

   **Confidence and second round.** Every panel gets a confidence score built from
   simple checks: a notched outline, a cut at a speech balloon, a paper-coloured gutter-like
   line inside it, little drawn border, tiny or sliver-shaped, or 1.5×/2.5× bigger than the
   book's median panel. Doubtful panels get a second, stricter search at full resolution
   for a straight gutter at any angle within ±12° of horizontal or vertical. A gutter counts
   only if the line is at least 90% paper with a dark border line close by on *both* sides,
   so caption boxes and horizons are not cut. If no gutter is found, the "gutter inside" and
   "bigger than usual" doubts are cleared. Small doubtful fragments (title lettering) are
   merged into the nearest panel, and slivers along the page edge (scan debris) are dropped.
   Panels that stay doubtful are listed for review. `--no-refine` skips this round.

   Each panel keeps its outline (the convex hull of its blob), so slanted and irregular panels
   are cut out exactly and slivers of neighbouring panels are painted white. Use `--rect-crop`
   to crop plain bounding boxes instead.
3. **Layout**: each panel must appear at least `--readable-scale` (default 0.8) times its printed
   size. The tool works this out from the device ppi and `--print-height`. Panels from the same
   row stay side by side when they fit. Otherwise they are stacked. Each Kindle page is filled
   with as many panels as fit, and then scaled up to use any leftover space.
4. **Text layer** (PDF only): each page is OCR'd with Tesseract, and every recognised word is
   placed as invisible text exactly over its lettering, following the panel's crop, scale and
   rotation. Long-pressing a word on the Kindle then selects it for dictionary lookup,
   highlighting and search. ALL-CAPS lettering is stored in lowercase, and words hyphenated
   across lines are joined, so lookups find dictionary entries. The language is set with
   `--lang` (default `en`), and `--no-ocr` turns this off. OCR adds about 1 s per page per
   CPU core. Sound effects and stylised lettering are mostly not recognised.
5. **Output**: grayscale JPEG pages (colour on Colorsoft or with `--color`), saved as PDF, CBZ or
   an image folder.

## Tuning

| Problem | Try |
|---|---|
| Text still too small | `-s 1.0` (or higher) |
| Too few panels per page | `-s 0.65` |
| Wide panels too small | `--rotate` (puts them in landscape on their own page) |
| Panels merged together | `--split-coverage 0.5`, `--threshold 30` |
| Panels split too much | `--split-coverage 0.2` |
| Washed-out e-ink | `--gamma 1.4` |
| Dictionary lookup missing words | check `--lang`; copy the PDF over USB (Send to Kindle can drop the text layer) |
| Neighbouring panels cut into an L-shaped panel | `--rect-crop` |
| Check detection | `--debug-dir dbg/` writes each page with the detected panel outlines, numbered in reading order (`3?` = still doubtful), plus `report.txt` with each panel's confidence and the reasons |

## Known limitations

- Panels merged **without any gutter** (art or balloons run straight from one panel into the
  next) can't be split; they're reported as doubtful (`much bigger than usual`).
- A gutter crossed by artwork, not only by balloons, can be missed.
- A balloon that crosses a slanted gutter is cut along the gutter, so its last letters can end
  up with the neighbouring panel.
