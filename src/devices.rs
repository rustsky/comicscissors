//! E-reader screen profiles.

use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Clone, Debug)]
pub struct Device {
    pub brand: String,
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub ppi: u32,
    pub color: bool,
}

// (brand, id, name, width, height, ppi, colour). Colour E Ink (Kaleido) screens list their
// black-and-white resolution; colour is shown at half of it.
const PROFILES: &[(&str, &str, &str, u32, u32, u32, bool)] = &[
    ("Kindle", "kindle-basic", "Kindle basic 4/5/7/8/10th gen, Kindle Touch (6\", 167 ppi)", 600, 800, 167, false),
    ("Kindle", "kindle-11", "Kindle 11th gen 2022 / Kindle 2024 (6\", 300 ppi)", 1072, 1448, 300, false),
    ("Kindle", "paperwhite-1", "Paperwhite 1/2 (2012-2013, 212 ppi)", 758, 1024, 212, false),
    ("Kindle", "paperwhite-3", "Paperwhite 3/4 (2015-2018), Voyage, Oasis 1 (300 ppi)", 1072, 1448, 300, false),
    ("Kindle", "paperwhite-5", "Paperwhite 5 / Signature Edition (2021, 6.8\")", 1236, 1648, 300, false),
    ("Kindle", "paperwhite-6", "Paperwhite 12th gen (2024, 7\")", 1264, 1680, 300, false),
    ("Kindle", "oasis", "Oasis 2/3 (2017-2019, 7\")", 1264, 1680, 300, false),
    ("Kindle", "colorsoft", "Kindle Colorsoft (2024, 7\", color)", 1264, 1680, 300, true),
    ("Kindle", "scribe", "Kindle Scribe (10.2\")", 1860, 2480, 300, false),
    ("Kindle", "dx", "Kindle DX (9.7\")", 824, 1200, 150, false),
    ("Kobo", "kobo-nia", "Kobo Nia (6\", 212 ppi)", 758, 1024, 212, false),
    ("Kobo", "kobo-clara", "Kobo Clara HD / 2E / BW (6\", 300 ppi)", 1072, 1448, 300, false),
    ("Kobo", "kobo-clara-colour", "Kobo Clara Colour (6\", color)", 1072, 1448, 300, true),
    ("Kobo", "kobo-libra", "Kobo Libra H2O / Libra 2 (7\", 300 ppi)", 1264, 1680, 300, false),
    ("Kobo", "kobo-libra-colour", "Kobo Libra Colour (7\", color)", 1264, 1680, 300, true),
    ("Kobo", "kobo-sage", "Kobo Forma / Sage (8\", 300 ppi)", 1440, 1920, 300, false),
    ("Kobo", "kobo-elipsa", "Kobo Elipsa / Elipsa 2E (10.3\", 227 ppi)", 1404, 1872, 227, false),
    ("PocketBook", "pocketbook-verse-pro", "PocketBook Verse Pro (6\", 300 ppi)", 1072, 1448, 300, false),
    ("PocketBook", "pocketbook-era", "PocketBook Era (7\", 300 ppi)", 1264, 1680, 300, false),
    ("PocketBook", "pocketbook-inkpad-4", "PocketBook InkPad 4 (7.8\", 300 ppi)", 1404, 1872, 300, false),
    ("PocketBook", "pocketbook-inkpad-color-3", "PocketBook InkPad Color 3 (7.8\", color)", 1404, 1872, 300, true),
    ("Boox", "boox-palma", "Boox Palma (6.13\", 300 ppi)", 824, 1648, 300, false),
    ("Boox", "boox-page", "Boox Page / Leaf 3 / Go 7 (7\", 300 ppi)", 1264, 1680, 300, false),
    ("Boox", "boox-note-air", "Boox Note Air 2/3, Tab Ultra (10.3\", 227 ppi)", 1404, 1872, 227, false),
    ("Boox", "boox-go-10", "Boox Go 10.3 (10.3\", 300 ppi)", 1860, 2480, 300, false),
    ("Tolino", "tolino-shine", "Tolino Shine 4 (6\", 300 ppi)", 1072, 1448, 300, false),
    ("Tolino", "tolino-vision", "Tolino Vision 6 (7\", 300 ppi)", 1264, 1680, 300, false),
    ("Tolino", "tolino-epos", "Tolino Epos 3 (8\", 300 ppi)", 1440, 1920, 300, false),
    ("reMarkable", "remarkable-2", "reMarkable 2 (10.3\", 226 ppi)", 1404, 1872, 226, false),
    ("reMarkable", "remarkable-pro", "reMarkable Paper Pro (11.8\", color)", 1620, 2160, 229, true),
    ("Nook", "nook-glowlight-4", "Nook GlowLight 4 (6\", 300 ppi)", 1072, 1448, 300, false),
];

pub fn all() -> Vec<Device> {
    PROFILES
        .iter()
        .map(|&(brand, id, name, width, height, ppi, color)| Device {
            brand: brand.into(),
            id: id.into(),
            name: name.into(),
            width,
            height,
            ppi,
            color,
        })
        .collect()
}

pub fn find(id: &str) -> Option<Device> {
    let id = id.to_ascii_lowercase();
    all().into_iter().find(|d| d.id == id)
}

pub fn print_list() {
    let mut brand = String::new();
    for d in all() {
        if d.brand != brand {
            println!("{}:", d.brand);
            brand = d.brand.clone();
        }
        println!("  {:<26} {:>4}x{:<4} {:>3} ppi  {}", d.id, d.width, d.height, d.ppi, d.name);
    }
}

/// Ask the user which e-reader they own.
pub fn prompt() -> Result<Device> {
    if !io::stdin().is_terminal() {
        bail!("no --device given and stdin is not a terminal; pass --device (see --list-devices)");
    }
    let devices = all();
    println!("Which e-reader will you read on? (other screens: --width, --height, --ppi)");
    let mut brand = "";
    for (i, d) in devices.iter().enumerate() {
        if d.brand != brand {
            println!("{}:", d.brand);
            brand = &d.brand;
        }
        println!("  {:>2}) {}  [{}x{}]", i + 1, d.name, d.width, d.height);
    }
    let stdin = io::stdin();
    loop {
        print!("Select 1-{}: ", devices.len());
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).context("reading selection")? == 0 {
            bail!("no device selected");
        }
        let line = line.trim();
        if let Ok(n) = line.parse::<usize>()
            && (1..=devices.len()).contains(&n)
        {
            return Ok(devices[n - 1].clone());
        }
        if let Some(d) = find(line) {
            return Ok(d);
        }
        println!("Invalid choice '{line}'.");
    }
}
