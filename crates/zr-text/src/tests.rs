use super::*;

type Str<'a> = (&'a str, Option<&'a str>);

/// Builds a synthetic `.csf`: labels with `(wide text, optional wave)` strings.
fn csf(version: i32, labels: &[(&str, &[Str])]) -> Vec<u8> {
    let mut out = b" FSC".to_vec();
    for v in [version, labels.len() as i32, 0, 0, 7] {
        out.extend(v.to_le_bytes());
    }
    for (label, strings) in labels {
        out.extend(b" LBL");
        out.extend((strings.len() as i32).to_le_bytes());
        out.extend((label.len() as i32).to_le_bytes());
        out.extend(label.as_bytes());
        for (text, wave) in *strings {
            let units: Vec<u16> = text.encode_utf16().collect();
            out.extend(if wave.is_some() { b"WRTS" } else { b" RTS" });
            out.extend((units.len() as i32).to_le_bytes());
            for u in units {
                out.extend((!u).to_le_bytes());
            }
            if let Some(w) = wave {
                out.extend((w.len() as i32).to_le_bytes());
                out.extend(w.as_bytes());
            }
        }
    }
    out
}

#[test]
fn csf_roundtrip_and_case_insensitive_lookup() {
    let data = csf(
        3,
        &[
            ("OBJECT:Crusader", &[("  Crusader   Tank ", None)]),
            ("GUI:Hello", &[("Hi \u{e9}\u{4e2d}", Some("hello01")), ("second ignored", None)]),
            ("GUI:Empty", &[]),
        ],
    );
    let t = StringTable::parse_csf(&data).unwrap();
    assert_eq!(t.len(), 3);
    assert_eq!(t.language_id, Some(7));
    assert_eq!(t.get("object:crusader"), Some("Crusader Tank"), "spaces stripped, any case");
    assert_eq!(t.get("OBJECT:CRUSADER"), Some("Crusader Tank"));
    let e = t.entry("gui:hello").unwrap();
    assert_eq!(e.text, "Hi \u{e9}\u{4e2d}");
    assert_eq!(e.speech.as_deref(), Some("hello01"));
    assert_eq!(t.get("GUI:Empty"), Some(""));
    assert_eq!(t.get("GUI:Missing"), None);
    let labels: Vec<_> = t.labels_with_prefix("GUI:").collect();
    assert_eq!(labels, ["GUI:Empty", "GUI:Hello"], "sorted order, case-sensitive prefix");
    assert_eq!(t.labels_with_prefix("gui:").count(), 0);
}

#[test]
fn csf_version_1_has_no_language() {
    let t = StringTable::parse_csf(&csf(1, &[("A", &[("x", None)])])).unwrap();
    assert_eq!(t.language_id, None);
}

#[test]
fn csf_rejects_garbage_without_panicking() {
    let good = csf(3, &[("LBL", &[("text", Some("w"))]), ("B", &[("more", None)])]);
    assert_eq!(StringTable::parse_csf(b""), Err(Error::Truncated { offset: 0 }));
    assert_eq!(StringTable::parse_csf(&good[..10]), Err(Error::Truncated { offset: 0 }));
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(StringTable::parse_csf(&bad), Err(Error::BadMagic));
    let mut bad = good.clone();
    bad[24] = b'X';
    assert_eq!(StringTable::parse_csf(&bad), Err(Error::BadLabelTag { offset: 24 }));
    // every truncation and every single-byte corruption is handled
    for n in 0..good.len() {
        let _ = StringTable::parse_csf(&good[..n]);
    }
    for i in 0..good.len() {
        for v in [0u8, 0xFF, 0x7F] {
            let mut b = good.clone();
            b[i] = v;
            let _ = StringTable::parse_csf(&b);
        }
    }
    // a huge declared length does not allocate
    let mut huge = csf(3, &[("A", &[("x", None)])]);
    huge[32..36].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(StringTable::parse_csf(&huge).is_err());
    let mut neg = csf(3, &[("A", &[("x", None)])]);
    neg[28..32].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(matches!(StringTable::parse_csf(&neg), Err(Error::BadLength { .. })));
    // trailing bytes shorter than a tag are ignored, like the engine's short read
    let mut tail = good.clone();
    tail.extend([1, 2, 3]);
    assert_eq!(StringTable::parse_csf(&tail).unwrap().len(), 2);
}

#[test]
fn strip_spaces_matches_engine() {
    let f = |s: &str| {
        let u: Vec<u16> = s.encode_utf16().collect();
        crate::csf::utf16(&crate::csf::strip_spaces(&u))
    };
    assert_eq!(f("  a   b  "), "a b");
    assert_eq!(f("a \n  b"), "a\nb");
    assert_eq!(f("a\t c"), "a\tc");
    assert_eq!(f("   "), "");
}

const STR: &[u8] = b"// header comment\r\n\
GUI:One\r\n\
\"Hello,   \\\"world\\\"\\nline two\" voice_1\r\n\
END\r\n\
\r\n\
gui:multi\r\n\
\"first\r\n\
 second\" = spk\r\n\
\"ignored\"\r\n\
end\r\n\
GUI:NoWave\r\n\
\"just text\"\r\n\
END\r\n";

#[test]
fn str_format() {
    let t = StringTable::parse_str(STR).unwrap();
    assert_eq!(t.len(), 3);
    let one = t.entry("GUI:ONE").unwrap();
    assert_eq!(one.text, "Hello, \"world\"\nline two");
    assert_eq!(one.speech.as_deref(), Some("voice_1e"), "trailing digit gets an `e`");
    let m = t.entry("GUI:MULTI").unwrap();
    assert_eq!(m.text, "first second", "newline inside a quote is a space; extra strings ignored");
    assert_eq!(m.speech.as_deref(), Some("spk"));
    assert_eq!(t.entry("gui:nowave").unwrap().speech, None);
    assert_eq!(t.get("GUI:NoWave"), Some("just text"));
}

#[test]
fn str_errors() {
    assert_eq!(
        StringTable::parse_str(b"GUI:A\n\"text\"\n"),
        Err(Error::UnexpectedEnd { label: "GUI:A".into() })
    );
    assert!(StringTable::parse_str(b"").unwrap().is_empty());
    // unterminated quote at EOF and arbitrary bytes never panic
    let _ = StringTable::parse_str(b"A\n\"never closed");
    let _ = StringTable::parse_str(&[0xFF, 0, b'\n', b'"', 0x80, b'\\']);
    let long = [b"A\n\"".as_slice(), &vec![b'x'; 50_000], b"\"\nEND\n"].concat();
    let t = StringTable::parse_str(&long).unwrap();
    assert!(t.get("A").unwrap().len() <= 10 * 1024, "quoted text is capped like the engine");
}

#[test]
fn game_text_map_overlay_and_fetch() {
    let main = StringTable::parse_str(b"A\n\"main a\"\nEND\n").unwrap();
    let map = StringTable::parse_str(b"A\n\"map a\"\nEND\nMAP:B\n\"map b\"\nEND\n").unwrap();
    let mut gt = GameText::new(main);
    assert_eq!(gt.fetch("MAP:B"), "MISSING: 'MAP:B'");
    gt.set_map_strings(map);
    assert_eq!(gt.fetch("a"), "main a", "main table wins");
    assert_eq!(gt.fetch("map:b"), "map b");
    assert_eq!(gt.fetch_or("nope", "fallback"), "fallback");
    assert_eq!(gt.labels_with_prefix("MAP:").collect::<Vec<_>>(), ["MAP:B"]);
    gt.clear_map_strings();
    assert_eq!(gt.get("MAP:B"), None);
}

#[test]
fn duplicate_labels_resolve_to_first() {
    let t = StringTable::parse_str(b"A\n\"one\"\nEND\na\n\"two\"\nEND\n").unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t.get("A"), Some("one"));
}

#[test]
fn patch_table_fills_labels_the_main_table_lacks() {
    let main = StringTable::parse_str(b"A\n\"main a\"\nEND\n").unwrap();
    let patch = StringTable::parse_str(b"GUI:CustomMission\n\"CUSTOM MISSION\"\nEND\nA\n\"patch a\"\nEND\n").unwrap();
    let mut gt = GameText::new(main);
    assert_eq!(gt.fetch("GUI:CustomMission"), "MISSING: 'GUI:CustomMission'");
    gt.patch = Some(patch);
    assert_eq!(gt.fetch("gui:custommission"), "CUSTOM MISSION");
    assert_eq!(gt.fetch("A"), "main a", "the main table wins over the patch");
    assert_eq!(gt.labels_with_prefix("GUI:").collect::<Vec<_>>(), ["GUI:CustomMission"]);
}

/// The retail install's CSF lacks `GUI:CustomMission`; `Data\Patch.str` supplies it. Skips without an install.
#[test]
fn retail_load_layers_patch_str_under_the_csf() {
    let Some(install) = zr_assets::Installs::detect().zero_hour else { return };
    let Ok(vfs) = zr_assets::Vfs::zero_hour(&install) else { return };
    let text = crate::load(&vfs, &crate::detect_language(&vfs)).expect("retail string file");
    assert!(text.main.get("GUI:CustomMission").is_none(), "the CSF has no CustomMission label");
    assert_eq!(text.fetch("GUI:CustomMission"), "CUSTOM MISSION");
}
