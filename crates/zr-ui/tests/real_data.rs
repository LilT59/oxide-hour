//! Real-install test. Skips (passes) gracefully when no Zero Hour install is found.

use zr_assets::{Installs, Vfs};
use zr_ui::make_report;

#[test]
fn real_install_wnd_layouts_and_mapped_images() {
    let Some(install) = Installs::detect().zero_hour else {
        eprintln!("skipping: no Zero Hour install found");
        return;
    };
    let Ok(vfs) = Vfs::zero_hour(&install) else {
        eprintln!("skipping: could not mount Zero Hour VFS");
        return;
    };
    let loaded = zr_ini::load_zero_hour(&vfs);
    let report = make_report(&vfs, &loaded.db, &loaded.language);

    assert!(report.wnd.files > 50, ".wnd files: {}", report.wnd.files);
    assert_eq!(report.wnd.failures, Vec::new(), "all shipped .wnd files should parse");
    assert_eq!(report.wnd.parsed, report.wnd.files);
    assert!(report.wnd.windows > 500, "windows: {}", report.wnd.windows);
    assert!(report.images.mapped_images > 500, "MappedImages: {}", report.images.mapped_images);
    // Retail is mostly self-contained; a broken folder/extension guess would tank this.
    assert!(
        report.images.resolved * 2 > report.images.referenced,
        "resolved {}/{} image refs",
        report.images.resolved,
        report.images.referenced
    );

    // The in-game command bar must parse and be a real tree.
    let bytes = vfs.read_archived(r"window\controlbar.wnd").expect("ControlBar.wnd in VFS");
    let f = zr_ui::parse(bytes).expect("ControlBar.wnd parses");
    assert_eq!(f.file_version, 2);
    assert!(!f.windows.is_empty());
    assert!(f.windows[0].children.len() > 5, "control bar has children");
    eprintln!("{}", report.summary());
}
