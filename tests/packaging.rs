//! The files `make install` and the .deb ship: desktop entry, icon, and
//! that they agree with the application id the app registers.

use std::path::Path;

use fennec::APP_ID;

fn desktop_entry() -> String {
    std::fs::read_to_string(format!("data/{APP_ID}.desktop")).expect("desktop file named after the app id")
}

fn key<'a>(entry: &'a str, name: &str) -> Option<&'a str> {
    entry.lines().find_map(|l| l.strip_prefix(&format!("{name}=")))
}

#[test]
fn the_desktop_entry_launches_the_installed_binary_with_its_icon() {
    let entry = desktop_entry();
    assert_eq!(key(&entry, "Exec"), Some("fennec"));
    assert_eq!(key(&entry, "Icon"), Some(APP_ID));
    assert_eq!(key(&entry, "StartupWMClass"), Some(APP_ID));
    assert!(Path::new(&format!("data/icons/hicolor/scalable/apps/{APP_ID}.svg")).exists());
    assert!(
        std::fs::read_to_string("Cargo.toml")
            .unwrap()
            .contains("name = \"fennec\""),
        "Exec names the package binary"
    );
}

#[test]
fn the_desktop_entry_validates() {
    let path = format!("data/{APP_ID}.desktop");
    match std::process::Command::new("desktop-file-validate")
        .arg(&path)
        .output()
    {
        Ok(out) => {
            let report =
                String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success() && report.trim().is_empty(), "{report}");
        }
        Err(_) => eprintln!("desktop-file-validate is not installed; skipping"),
    }
}

#[test]
fn the_icon_is_a_square_svg() {
    let svg = std::fs::read_to_string(format!("data/icons/hicolor/scalable/apps/{APP_ID}.svg")).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 128 128\""));
}

#[test]
fn install_targets_ship_every_packaged_file() {
    let make = std::fs::read_to_string("Makefile").unwrap();
    let deb = std::fs::read_to_string("scripts/build-deb.sh").unwrap();
    for needed in ["target/release/fennec ", "fennec-bench", ".desktop", ".svg"] {
        assert!(make.contains(needed), "Makefile misses {needed}");
        assert!(deb.contains(needed), "build-deb.sh misses {needed}");
    }
}

#[test]
fn make_install_points_the_launcher_at_the_installed_binary() {
    // Desktop launchers often run without ~/.local/bin on PATH.
    let make = std::fs::read_to_string("Makefile").unwrap();
    assert!(
        make.contains("s|^Exec=fennec|Exec=$(PREFIX)/bin/fennec|"),
        "{make}"
    );
}

#[test]
fn the_package_brings_the_mockup_fonts_and_the_icon_licence() {
    let deb = std::fs::read_to_string("scripts/build-deb.sh").unwrap();
    let recommends = deb
        .lines()
        .find(|l| l.starts_with("Recommends:"))
        .expect("a Recommends line");
    assert!(recommends.contains("fonts-ibm-plex"), "{recommends}");
    assert!(
        deb.contains("data/icons/ui/LICENSE"),
        "the Lucide licence is not shipped"
    );
}

#[test]
fn both_installs_ship_the_document_serif() {
    let make = std::fs::read_to_string("Makefile").unwrap();
    let deb = std::fs::read_to_string("scripts/build-deb.sh").unwrap();
    for f in [
        "SourceSerif4Variable-Roman.ttf",
        "SourceSerif4Variable-Italic.ttf",
    ] {
        assert!(
            std::path::Path::new("data/fonts").join(f).exists(),
            "{f} is missing"
        );
        assert!(make.contains("data/fonts"), "make install leaves out the fonts");
        assert!(deb.contains("data/fonts"), "the package leaves out the fonts");
    }
    assert!(
        deb.contains("LICENSE-SourceSerif4.md"),
        "the font licence is not shipped"
    );
}
