//! Synthetic INI parse / resolution tests (no install needed).

use std::collections::BTreeSet;

use zr_audio::{sound_type, AudioDatabase, AudioPriority, AudioType};
use zr_ini::IniDb;

fn parse_db(ini: &str) -> IniDb {
    let mut db = IniDb::new();
    db.extend(zr_ini::parse("test.ini", ini).expect("synthetic INI parses"));
    db
}

#[test]
fn defaults_seed_events_and_tracks() {
    let db = parse_db(
        r#"
AudioEvent DefaultSoundEffect
  Volume = 100
  Priority = high
  MinVolume = 40
  Control = random
  Limit = 4
  Type = ui player
End
AudioEvent Explosion
  Sounds = a b c
  PitchShift = -10 10
  VolumeShift = -20
  Type = world shrouded everyone
End
MusicTrack DefaultMusicTrack
  Filename = NoFilename
End
MusicTrack Track1
  Filename = USA_09.mp3
End
DialogEvent DefaultDialog
  Volume = 100
  Type = ui everyone
End
DialogEvent Eva1
  Filename = egallyu.wav
End
MiscAudio
  GUIClickSound = GUIClick
End
"#,
    );
    let db = AudioDatabase::from_ini(&db, "english");

    assert_eq!(db.counts(), (2, 2, 2), "one default + one user per kind");

    let ex = db.get("Explosion").unwrap();
    assert_eq!(ex.sound_type, AudioType::SoundEffect);
    assert_eq!(ex.priority, AudioPriority::High, "seeded from DefaultSoundEffect");
    assert_eq!(ex.limit, 4, "seeded from DefaultSoundEffect");
    assert_eq!(ex.min_volume, 0.4);
    assert_eq!(ex.volume, 1.0);
    assert_eq!(ex.volume_shift, -0.2);
    assert_eq!((ex.pitch_shift_min, ex.pitch_shift_max), (0.9, 1.1));
    assert_eq!(ex.type_bits, sound_type::WORLD | sound_type::SHROUDED | sound_type::EVERYONE);
    assert_eq!(ex.sounds, ["a", "b", "c"]);

    let track = db.get("Track1").unwrap();
    assert_eq!(track.sound_type, AudioType::Music);
    assert_eq!(track.filename.as_deref(), Some("USA_09.mp3"));
    assert_eq!(db.get("Eva1").unwrap().sound_type, AudioType::Streaming);
    assert_eq!(db.misc_audio, [("GUIClickSound".to_owned(), "GUIClick".to_owned())]);
    assert!(db.get("GUIClick").is_none());
}

#[test]
fn later_definition_overrides_field_by_field() {
    let db = parse_db(
        r#"
AudioEvent Boom
  Sounds = one
  Volume = 50
End
AudioEvent Boom
  Sounds = two
End
"#,
    );
    let db = AudioDatabase::from_ini(&db, "english");
    let e = db.get("Boom").unwrap();
    assert_eq!(e.sounds, ["two"], "list fields replace");
    assert_eq!(e.volume, 0.5, "untouched fields survive");
    assert_eq!(db.counts().0, 1);
}

#[test]
fn resolution_prefers_localized_then_base() {
    let db = parse_db(
        r#"
AudioSettings
  AudioRoot = Data\Audio
  SoundsFolder = Sounds
  MusicFolder = Tracks
  StreamingFolder = Speech
  SoundsExtension = wav
End
AudioEvent Explosion
  Sounds = boom
End
DialogEvent Eva1
  Filename = egallyu.wav
End
"#,
    );
    let db = AudioDatabase::from_ini(&db, "english");
    let mut files = BTreeSet::new();
    files.insert(zr_assets::normalize_path(r"Data\Audio\Sounds\boom.wav"));
    files.insert(zr_assets::normalize_path(r"Data\Audio\Speech\english\egallyu.wav"));
    let exists = |p: &str| files.contains(&zr_assets::normalize_path(p));

    let boom = db.resolve_reference(AudioType::SoundEffect, "boom", &exists).unwrap();
    assert!(!boom.localized);
    assert_eq!(zr_assets::normalize_path(&boom.path), r"data\audio\sounds\boom.wav");
    assert_eq!(boom.filename, "boom.wav");

    let eva = db.resolve_reference(AudioType::Streaming, "egallyu.wav", &exists).unwrap();
    assert!(eva.localized, "localized path wins when present");
    assert!(zr_assets::normalize_path(&eva.path).contains(r"speech\english\egallyu.wav"));

    assert!(db.resolve_reference(AudioType::SoundEffect, "missing", &exists).is_none());
}
