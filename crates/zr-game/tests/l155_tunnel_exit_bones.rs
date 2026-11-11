//! Lane 155 (wave 18): a GLA tunnel network's `ExitStart` / `ExitEnd` bones (`OpenContain::exitObjectViaDoor`,
//! `OpenContain.cpp:1038-1057`) resolved from the install's W3D model, which the tunnel's spawns leave through. The
//! values are the bind pose of `GLATunnelNetwork`'s model (`UBUndTunn`) in object space. Skips when no install is found.

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::template::TemplateSource;

/// `ExitStart` (3.5225, -0.0779, -0.5001) and `ExitEnd` (34.4925, -0.0132, 0.0) are the model's bind-pose bones. The
/// C++ spawns leave at the start bone with the tunnel at (1512.3408, 1960.6038, 18.75) and an orientation that puts it
/// at (1509.7949, 1958.1681, 18.2499) (`zr-refs` AH7 s1 f1906). Revert check: without the tunnel branch in
/// `firepoints::garrison_fire_points` the bone list is empty and the test fails.
#[test]
fn a_gla_tunnel_network_has_its_exit_bones() {
    let Some(zh) = Installs::detect().zero_hour else { return };
    let Ok(vfs) = Vfs::zero_hour(&zh) else { return };
    let data = GameData::load(&vfs);
    let Some(t) = data.store.find_template("GLATunnelNetwork") else { return };
    let contain = t.contain.as_ref().expect("a tunnel network has a contain module");
    let [(Some(start), Some(end))] = contain.fire_points.exit_bones.as_slice() else {
        panic!("one exit path with both bones, got {:?}", contain.fire_points.exit_bones);
    };
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    assert!(close(start.x, 3.5225) && close(start.y, -0.0779) && close(start.z, -0.5001), "ExitStart {start:?}");
    assert!(close(end.x, 34.4925) && close(end.y, -0.0132) && close(end.z, 0.0), "ExitEnd {end:?}");
}
