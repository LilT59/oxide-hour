//! `PolygonTriggers` chunk (versions 1..=4).
//!
//! Ported from `PolygonTrigger::ParsePolygonTriggersDataChunk`
//! (`Core/GameEngine/Source/GameLogic/Map/PolygonTrigger.cpp`); versions in
//! `Core/GameEngine/Include/Common/MapReaderWriterInfo.h`
//! (2 added the water flag, 3 river flag + start, 4 layer name).
//!
//! ## Water (for a future water renderer)
//!
//! Standing water is defined entirely by triggers with [`PolygonTrigger::is_water`]:
//! - Water height is the **z of the first point** (`TerrainLogic::getWaterHeight`,
//!   `TerrainLogic.cpp`; `getWaterHandle` picks, among water triggers containing the point,
//!   the one with the highest first-point z).
//! - Plain water areas are drawn by `WaterRenderObjClass::renderWater` (`W3DWater.cpp`) as a
//!   fan of quads around point 0 at each point's own z; `is_river` triggers go through
//!   `drawRiverWater` instead (`river_start` selects the point index where the river's
//!   flow edge begins).
//! - Look and feel come from INI: `WaterTransparency` (`TheWaterTransparency`:
//!   `TransparentWaterDepth`, `MinWaterOpacity`, `StandingWaterColor`, `StandingWaterTexture`,
//!   `AdditiveBlending`, ...) and `WaterSet` per time of day (sky, sun, textures). A separate
//!   legacy "grid" water mesh (`W3DTerrainVisual::getWaterGridHeight`) is only driven by scripts.
//! - Version 1 chunks predate water areas: the engine appends an automatic water trigger at
//!   z = 7 sized from `GlobalData::m_waterExtentX/Y`. We don't invent it here; check
//!   [`PolygonTriggers::version`] and use `GlobalData` `WaterPositionZ/ExtentX/ExtentY`.

use crate::chunk::{Chunk, ChunkError, Names, Result};
use crate::map::count;

/// One polygon trigger. Coordinates are integer world units (the original's `ICoord3D`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolygonTrigger {
    pub name: String,
    /// Editor layer (version >= 4, else empty).
    pub layer: String,
    pub id: i32,
    /// Water area (version >= 2).
    pub is_water: bool,
    /// River (version >= 3).
    pub is_river: bool,
    /// Index of the point where the river starts (version >= 3).
    pub river_start: i32,
    pub points: Vec<[i32; 3]>,
}

impl PolygonTrigger {
    /// Water surface height: z of point 0 (`TerrainLogic::getWaterHeight`).
    pub fn water_height(&self) -> Option<f32> {
        self.points.first().map(|p| p[2] as f32)
    }

    /// The engine deletes triggers with fewer than 2 points when loading (non-retail-CRC
    /// builds); the renderer additionally needs more than 2 for water.
    pub fn is_valid(&self) -> bool {
        self.points.len() >= 2
    }

    /// `PolygonTrigger::pointInTrigger`: bounding box reject, then even-odd ray cast.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let Some(first) = self.points.first() else { return false };
        let (mut lo, mut hi) = ([first[0], first[1]], [first[0], first[1]]);
        for p in &self.points {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        if x < lo[0] || y < lo[1] || x > hi[0] || y > hi[1] {
            return false;
        }
        let mut inside = false;
        for (i, p1) in self.points.iter().enumerate() {
            let p2 = &self.points[(i + 1) % self.points.len()];
            if p1[1] == p2[1] || (p1[1] < y && p2[1] < y) || (p1[1] >= y && p2[1] >= y) || (p1[0] < x && p2[0] < x) {
                continue;
            }
            let dy = p2[1].wrapping_sub(p1[1]);
            let dx = p2[0].wrapping_sub(p1[0]);
            let ix = p1[0] as f32 + dx.wrapping_mul(y.wrapping_sub(p1[1])) as f32 / dy as f32;
            if ix >= x as f32 {
                inside = !inside;
            }
        }
        inside
    }
}

/// Decoded `PolygonTriggers` chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolygonTriggers {
    pub version: u16,
    pub triggers: Vec<PolygonTrigger>,
}

impl PolygonTriggers {
    pub(crate) fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        // Per trigger: name (>= 2) + id (4) + numPoints (4), plus flag bytes from v2.
        let min = 10 + usize::from(c.version >= 2) + if c.version >= 3 { 5 } else { 0 } + if c.version >= 4 { 2 } else { 0 };
        let n = count(&mut r, min)?;
        let mut triggers = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.ascii()?;
            let layer = if c.version >= 4 { r.ascii()? } else { String::new() };
            let id = r.i32()?;
            let is_water = if c.version >= 2 { r.bool()? } else { false };
            let (mut is_river, mut river_start) = (false, 0);
            if c.version >= 3 {
                is_river = r.bool()?;
                river_start = r.i32()?;
            }
            let points = count(&mut r, 12).map_err(|e| match e {
                ChunkError::Corrupt(_) => ChunkError::Corrupt("polygon point count exceeds chunk"),
                e => e,
            })?;
            let points = (0..points).map(|_| Ok([r.i32()?, r.i32()?, r.i32()?])).collect::<Result<_>>()?;
            triggers.push(PolygonTrigger { name, layer, id, is_water, is_river, river_start, points });
        }
        Ok(Self { version: c.version, triggers })
    }

    /// Triggers flagged as water areas, rivers included (what `renderWater` iterates).
    pub fn water_areas(&self) -> impl Iterator<Item = &PolygonTrigger> {
        self.triggers.iter().filter(|t| t.is_water)
    }

    /// Water areas that are rivers.
    pub fn rivers(&self) -> impl Iterator<Item = &PolygonTrigger> {
        self.water_areas().filter(|t| t.is_river)
    }

    pub fn by_id(&self, id: i32) -> Option<&PolygonTrigger> {
        self.triggers.iter().find(|t| t.id == id)
    }

    /// Case-sensitive name lookup (`getPolygonTriggerByName` style).
    pub fn by_name(&self, name: &str) -> Option<&PolygonTrigger> {
        self.triggers.iter().find(|t| t.name == name)
    }

    /// `TerrainLogic::getWaterHandle` without the grid mesh: among water areas containing
    /// `(x, y)`, the one whose first point has the highest z (ties go to the later one).
    pub fn water_at(&self, x: f32, y: f32) -> Option<&PolygonTrigger> {
        let (ix, iy) = ((x + 0.5).floor() as i32, (y + 0.5).floor() as i32);
        let mut best: Option<(&PolygonTrigger, i32)> = None;
        for t in self.water_areas().filter(|t| t.contains(ix, iy)) {
            let z = t.points.first()?[2];
            if best.is_none_or(|(_, bz)| z >= bz) {
                best = Some((t, z));
            }
        }
        best.map(|(t, _)| t)
    }
}

#[cfg(test)]
mod tests {
    use crate::chunk::test_util::*;
    use crate::map::MapFile;

    const NAMES: &[(&str, u32)] = &[("PolygonTriggers", 1)];

    fn trig(ver: u16, name: &str, id: i32, water: bool, river: bool, pts: &[[i32; 3]]) -> Vec<u8> {
        let mut d = ascii(name);
        if ver >= 4 {
            d.extend(ascii("layer"));
        }
        d.extend(i32s(&[id]));
        if ver >= 2 {
            d.push(water as u8);
        }
        if ver >= 3 {
            d.push(river as u8);
            d.extend(i32s(&[2]));
        }
        d.extend(i32s(&[pts.len() as i32]));
        for p in pts {
            d.extend(i32s(p));
        }
        d
    }

    fn chunk_for(ver: u16, trigs: &[Vec<u8>]) -> Vec<u8> {
        let mut d = i32s(&[trigs.len() as i32]);
        for t in trigs {
            d.extend(t);
        }
        file(NAMES, &chunk(1, ver, &d))
    }

    const SQUARE: [[i32; 3]; 4] = [[0, 0, 30], [100, 0, 30], [100, 100, 30], [0, 100, 30]];

    #[test]
    fn versions() {
        for ver in 1..=4u16 {
            let f = chunk_for(ver, &[trig(ver, "A", 5, true, true, &SQUARE), trig(ver, "B", 6, false, false, &SQUARE[..1])]);
            let t = MapFile::parse(&f).unwrap().triggers.unwrap();
            assert_eq!((t.version, t.triggers.len()), (ver, 2));
            let a = &t.triggers[0];
            assert_eq!((a.name.as_str(), a.id, a.points.len()), ("A", 5, 4));
            assert_eq!(a.is_water, ver >= 2);
            assert_eq!(a.is_river, ver >= 3);
            assert_eq!(a.river_start, if ver >= 3 { 2 } else { 0 });
            assert_eq!(a.layer, if ver >= 4 { "layer" } else { "" });
            assert_eq!(t.water_areas().count(), usize::from(ver >= 2));
            assert_eq!(t.rivers().count(), usize::from(ver >= 3));
            assert!(!t.triggers[1].is_valid());
        }
    }

    #[test]
    fn geometry() {
        let f = chunk_for(4, &[trig(4, "lo", 1, true, false, &SQUARE), trig(4, "hi", 2, true, false, &SQUARE.map(|p| [p[0] / 2, p[1] / 2, 50]))]);
        let t = MapFile::parse(&f).unwrap().triggers.unwrap();
        assert_eq!(t.by_id(1).unwrap().water_height(), Some(30.0));
        assert!(t.by_name("hi").unwrap().contains(10, 10));
        assert!(!t.by_name("hi").unwrap().contains(60, 10));
        assert_eq!(t.water_at(10.0, 10.0).unwrap().name, "hi");
        assert_eq!(t.water_at(80.0, 80.0).unwrap().name, "lo");
        assert!(t.water_at(200.0, 10.0).is_none());
    }

    #[test]
    fn hostile_input_is_an_error() {
        let ok = chunk_for(4, &[trig(4, "A", 5, true, false, &SQUARE)]);
        for n in 0..ok.len() {
            let _ = MapFile::parse(&ok[..n]);
        }
        // huge trigger count / point count
        let m = MapFile::parse(&file(NAMES, &chunk(1, 4, &i32s(&[i32::MAX])))).unwrap();
        assert!(m.triggers.is_none() && m.decode_errors.len() == 1);
        let mut d = i32s(&[1]);
        d.extend(ascii("x"));
        d.extend(ascii(""));
        d.extend(i32s(&[1]));
        d.extend([0, 0]);
        d.extend(i32s(&[0, i32::MAX]));
        let m = MapFile::parse(&file(NAMES, &chunk(1, 4, &d))).unwrap();
        assert_eq!(m.decode_errors.len(), 1);
        let m = MapFile::parse(&file(NAMES, &chunk(1, 4, &i32s(&[-1])))).unwrap();
        assert_eq!(m.decode_errors.len(), 1);
    }
}
