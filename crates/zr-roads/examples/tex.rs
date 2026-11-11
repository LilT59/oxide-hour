use zr_assets::{Installs, Vfs};
fn main() {
    let install = Installs::detect().zero_hour.unwrap();
    let vfs = Vfs::zero_hour(&install).unwrap();
    for n in std::env::args().skip(1) {
        match zr_w3d::load_texture(&vfs, &n) {
            Ok(t) => println!("{n}: {}x{}", t.width, t.height),
            Err(e) => println!("{n}: ERR {e}"),
        }
    }
}
