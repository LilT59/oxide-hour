//! `Matrix3D` (`Core/Libraries/Source/WWVegas/WWMath/matrix3d.h`): the world transform every `Thing` keeps as
//! `m_transform` (`Thing.h`). A 3x4 affine matrix whose fourth row is implicitly `(0, 0, 0, 1)`; column 3 is the
//! translation. The object's position and heading are read from it: the position is its translation
//! (`Thing::setTransformMatrix`, `Thing.cpp:263-273`), the heading its `Get_Z_Rotation` (`matrix3d.cpp:285`).
//!
//! The build the comparison runs is 32-bit MSVC with no `/arch` flag, so its scalar `float` math is SSE2 and every
//! `+ - * /` rounds to `f32` (`_M_IX86_FP == 2`). `Matrix3D::mul` is the inline `submul` form (`matrix3d.h:1571`,
//! `ALLOW_TEMPORARIES` undefined); the `x87` assembly in `matrix3d.cpp` sits inside `#if 0` (`:746-1034`). No FMA
//! is emitted, so the operations below keep the C++ operand order and each intermediate is an `f32`.

use crate::math::{cos, sin, Coord3D};

/// `Matrix3D`: `row[i]` is `Row[i]` (`Vector4`: X, Y, Z, W), so `row[i][3]` is the translation component `i`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix3D {
    pub row: [[f32; 4]; 3],
}

impl Matrix3D {
    /// `Matrix3D::Make_Identity` (`matrix3d.h`).
    pub const fn identity() -> Self {
        Self { row: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] }
    }

    /// The identity with the translation column set to `pos` (the translation `Thing::setPosition` writes).
    pub const fn from_translation(pos: Coord3D) -> Self {
        Self { row: [[1.0, 0.0, 0.0, pos.x], [0.0, 1.0, 0.0, pos.y], [0.0, 0.0, 1.0, pos.z]] }
    }

    /// `Get_X_Translation` / `Get_Y_Translation` / `Get_Z_Translation` (`matrix3d.h`).
    pub fn translation(&self) -> Coord3D {
        Coord3D::new(self.row[0][3], self.row[1][3], self.row[2][3])
    }

    /// `Set_X_Translation` / `Set_Y_Translation` / `Set_Z_Translation` (`Thing::setPosition`, `Thing.cpp:184-186`).
    pub fn set_translation(&mut self, pos: Coord3D) {
        self.row[0][3] = pos.x;
        self.row[1][3] = pos.y;
        self.row[2][3] = pos.z;
    }

    /// `Matrix3D::Translate(x, y, z)` (`matrix3d.h:666-672`): post-multiplies by a translation, so each row's
    /// translation gains the row's rotation applied to `(x, y, z)`.
    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        for r in &mut self.row {
            r[3] += r[0] * x + r[1] * y + r[2] * z;
        }
    }

    /// `Matrix3D::In_Place_Pre_Rotate_Z(theta)` (`matrix3d.h:1343-1365`): pre-multiplies the rotation part (rows 0
    /// and 1, columns 0 to 2 only) by a rotation about Z. `cosf`/`sinf` are the C runtime's float functions.
    pub fn in_place_pre_rotate_z(&mut self, theta: f32) {
        let c = cos(theta);
        let s = sin(theta);
        for j in 0..3 {
            let (t1, t2) = (self.row[0][j], self.row[1][j]);
            self.row[0][j] = c * t1 - s * t2;
            self.row[1][j] = s * t1 + c * t2;
        }
    }

    /// `Matrix3D::In_Place_Pre_Rotate_X(theta)` (`matrix3d.h:1275-1294`): rows 1 and 2 of the rotation part.
    pub fn in_place_pre_rotate_x(&mut self, theta: f32) {
        let (c, s) = (cos(theta), sin(theta));
        for j in 0..3 {
            let (t1, t2) = (self.row[1][j], self.row[2][j]);
            self.row[1][j] = c * t1 - s * t2;
            self.row[2][j] = s * t1 + c * t2;
        }
    }

    /// `Matrix3D::In_Place_Pre_Rotate_Y(theta)` (`matrix3d.h:1309-1328`): rows 0 and 2 of the rotation part.
    pub fn in_place_pre_rotate_y(&mut self, theta: f32) {
        let (c, s) = (cos(theta), sin(theta));
        for j in 0..3 {
            let (t1, t2) = (self.row[0][j], self.row[2][j]);
            self.row[0][j] = c * t1 + s * t2;
            self.row[2][j] = -s * t1 + c * t2;
        }
    }

    /// `Matrix3D::Rotate_X(s, c)` (`matrix3d.h:803`, `:766` for the angle form): post-multiplies by a rotation about X,
    /// acting on columns 1 and 2. The translation column is untouched.
    pub fn rotate_x(&mut self, s: f32, c: f32) {
        for r in &mut self.row {
            let (t1, t2) = (r[1], r[2]);
            r[1] = c * t1 + s * t2;
            r[2] = -s * t1 + c * t2;
        }
    }

    /// `Matrix3D::Rotate_Y(s, c)` (`matrix3d.h:870`, `:834` for the angle form): post-multiplies by a rotation about Y,
    /// acting on columns 0 and 2.
    pub fn rotate_y(&mut self, s: f32, c: f32) {
        for r in &mut self.row {
            let (t1, t2) = (r[0], r[2]);
            r[0] = c * t1 - s * t2;
            r[2] = s * t1 + c * t2;
        }
    }

    /// `Matrix3D::Rotate_Z(s, c)` (`matrix3d.h:937`, `:901` for the angle form): post-multiplies by a rotation about Z,
    /// acting on columns 0 and 1. This is how the physics integrates a yaw rate (`PhysicsUpdate.cpp:749`).
    pub fn rotate_z(&mut self, s: f32, c: f32) {
        for r in &mut self.row {
            let (t1, t2) = (r[0], r[1]);
            r[0] = c * t1 + s * t2;
            r[1] = -s * t1 + c * t2;
        }
    }

    /// `Matrix3D::Rotate_Z(theta)` (`matrix3d.h:901`): the angle form of [`rotate_z`](Self::rotate_z).
    pub fn rotate_z_angle(&mut self, theta: f32) {
        self.rotate_z(sin(theta), cos(theta));
    }

    /// The X axis of the rotation, `Matrix3D::Get_X_Vector` (`(Row[0][0], Row[1][0], Row[2][0])`), which
    /// `tryToOrientInThisDirection3D` reads as the object's current direction (`Locomotor.cpp:174`).
    pub fn x_vector(&self) -> Coord3D {
        Coord3D::new(self.row[0][0], self.row[1][0], self.row[2][0])
    }

    /// `Matrix3D::buildTransformMatrix(pos, dir)` (`matrix3d.cpp:412-441`): a matrix at `pos` whose X axis points along
    /// the unit vector `dir`. Yaw is taken from the projection on the XY plane, pitch from `dir.z`.
    pub fn build_transform(pos: Coord3D, dir: Coord3D) -> Matrix3D {
        let len2 = (dir.x * dir.x + dir.y * dir.y).sqrt();
        let sinp = dir.z;
        let cosp = len2;
        let (siny, cosy) = if len2 != 0.0 { (dir.y / len2, dir.x / len2) } else { (0.0, 1.0) };
        let mut m = Matrix3D::identity();
        m.translate(pos.x, pos.y, pos.z);
        // Yaw rotation to the projection of the target in the x-y plane, then the pitch (`Rotate_Y(-sinp, cosp)`).
        m.rotate_z(siny, cosy);
        m.rotate_y(-sinp, cosp);
        m
    }

    /// `Matrix3D::Set(const Vector3 & axis, float angle)` (`matrix3d.h:567-611`): the rotation about a unit `axis` with
    /// the translation zeroed (the matrix constructor `Matrix3D(axis, angle)`).
    pub fn axis_angle(axis: Coord3D, angle: f32) -> Matrix3D {
        let c = cos(angle);
        let s = sin(angle);
        let (ax, ay, az) = (axis.x, axis.y, axis.z);
        Matrix3D {
            row: [
                [ax * ax + c * (1.0 - ax * ax), ax * ay * (1.0 - c) - az * s, az * ax * (1.0 - c) + ay * s, 0.0],
                [ax * ay * (1.0 - c) + az * s, ay * ay + c * (1.0 - ay * ay), ay * az * (1.0 - c) - ax * s, 0.0],
                [az * ax * (1.0 - c) - ay * s, ay * az * (1.0 - c) + ax * s, az * az + c * (1.0 - az * az), 0.0],
            ],
        }
    }

    /// `Matrix3D::Rotate_Vector(const Vector3 &)` (`matrix3d.h:1772-1776`): the rotation part applied to `v`.
    pub fn rotate_vector(&self, v: Coord3D) -> Coord3D {
        let a = &self.row;
        Coord3D::new(
            a[0][0] * v.x + a[0][1] * v.y + a[0][2] * v.z,
            a[1][0] * v.x + a[1][1] * v.y + a[1][2] * v.z,
            a[2][0] * v.x + a[2][1] * v.y + a[2][2] * v.z,
        )
    }

    /// `Matrix3D::mul(A, B)` = `A * B` (`matrix3d.h:1571-1606`). Each element is `submul` (`matrix3d.h:1512`):
    /// `row.X*t1 + row.Y*t2 + row.Z*t3`, summed left to right; the translation column adds `A`'s translation last.
    pub fn mul(a: &Matrix3D, b: &Matrix3D) -> Matrix3D {
        let mut out = Matrix3D::identity();
        for i in 0..3 {
            let ar = a.row[i];
            for j in 0..3 {
                out.row[i][j] = ar[0] * b.row[0][j] + ar[1] * b.row[1][j] + ar[2] * b.row[2][j];
            }
            out.row[i][3] = (ar[0] * b.row[0][3] + ar[1] * b.row[1][3] + ar[2] * b.row[2][3]) + ar[3];
        }
        out
    }

    /// `Matrix3D::Get_Z_Rotation` (`matrix3d.cpp:285`): `WWMath::Atan2(Row[1][0], Row[0][0])`, which is
    /// `static_cast<float>(atan2(double, double))` (`wwmath.h:137`).
    pub fn z_rotation(&self) -> f32 {
        libm::atan2(self.row[1][0] as f64, self.row[0][0] as f64) as f32
    }

    /// The matrix `Thing::setOrientation` builds (`Thing.cpp:211-244`): `x`, `y`, `z` are the columns of the
    /// rotation, with `z = (0, 0, 1)`, `u = (Cos(angle), Sin(angle), 0)`, `y = z x u` and `x = y x z`, then
    /// `Set(x.x, y.x, z.x, pos.x, ...)`. The cross products use the `Coord3D::crossProduct` formulas
    /// (`Core/Libraries/Include/Lib/Coord3D.h:50-55`), so the signed zeros match the engine.
    pub fn from_orientation(angle: f32, pos: Coord3D) -> Matrix3D {
        let z = Coord3D::new(0.0, 0.0, 1.0);
        let u = Coord3D::new(cos(angle), sin(angle), 0.0);
        let y = cross(z, u);
        let x = cross(y, z);
        Matrix3D {
            row: [
                [x.x, y.x, z.x, pos.x],
                [x.y, y.y, z.y, pos.y],
                [x.z, y.z, z.z, pos.z],
            ],
        }
    }
}

/// `Coord3D::crossProduct(a, b, r)` (`Coord3D.h:50-55`).
fn cross(a: Coord3D, b: Coord3D) -> Coord3D {
    Coord3D::new(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rows: [[f32; 4]; 3]) -> Matrix3D {
        Matrix3D { row: rows }
    }

    /// `Make_Identity` then `Translate(1,2,0)`: the translation is `(1, 2, 0)` (rotation untouched).
    #[test]
    fn translate_on_identity_sets_the_translation() {
        let mut t = Matrix3D::identity();
        t.translate(1.0, 2.0, 0.0);
        assert_eq!(t, m([[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 2.0], [0.0, 0.0, 1.0, 0.0]]));
    }

    /// `Translate` post-multiplies: the increment is the rotation applied to `(x, y, z)`. A 90-degree
    /// rotation about Z maps `(1, 0)` to `(0, 1)`, so `Translate(1, 0, 0)` adds `(0, 1, 0)`.
    #[test]
    fn translate_applies_the_rotation_to_the_offset() {
        let mut t = m([[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        t.translate(1.0, 0.0, 0.0);
        assert_eq!(t.row[0][3], 0.0);
        assert_eq!(t.row[1][3], 1.0);
        assert_eq!(t.row[2][3], 0.0);
    }

    /// `In_Place_Pre_Rotate_Z(pi/2)` on the identity: rows 0 and 1 become `(0, -1)` and `(1, 0)`. Column 3 and row 2
    /// are not touched.
    #[test]
    fn pre_rotate_z_rotates_the_rotation_rows_only() {
        let mut t = Matrix3D::from_translation(Coord3D::new(5.0, 6.0, 7.0));
        t.in_place_pre_rotate_z(std::f32::consts::FRAC_PI_2);
        assert!((t.row[0][0]).abs() < 1e-6 && (t.row[0][1] + 1.0).abs() < 1e-6);
        assert!((t.row[1][0] - 1.0).abs() < 1e-6 && (t.row[1][1]).abs() < 1e-6);
        assert_eq!(t.row[2], [0.0, 0.0, 1.0, 7.0]);
        assert_eq!(t.row[0][3], 5.0);
        assert_eq!(t.row[1][3], 6.0);
    }

    /// `mul` is `A * B` with the translation column of `A` added last. `A = Translate(3, 4)`, `B = identity` gives
    /// `A`; `A * B` with a rotated `B` is checked element by element.
    #[test]
    fn mul_is_row_times_column_with_a_translation_term() {
        let a = Matrix3D::from_translation(Coord3D::new(3.0, 4.0, 0.0));
        let b = m([[0.0, -1.0, 0.0, 10.0], [1.0, 0.0, 0.0, 20.0], [0.0, 0.0, 1.0, 30.0]]);
        let p = Matrix3D::mul(&a, &b);
        assert_eq!(p.row[0], [0.0, -1.0, 0.0, 13.0]);
        assert_eq!(p.row[1], [1.0, 0.0, 0.0, 24.0]);
        assert_eq!(p.row[2], [0.0, 0.0, 1.0, 30.0]);
    }

    /// `Get_Z_Rotation` is `atan2(Row[1][0], Row[0][0])`: a heading of `0.5` read back from `from_orientation`.
    #[test]
    fn z_rotation_reads_back_the_heading() {
        let t = Matrix3D::from_orientation(0.5, Coord3D::ZERO);
        assert_eq!(t.z_rotation(), 0.5f32);
        // Row 0 is (cos, -sin, 0): the signed pairs of the rotation.
        assert_eq!(t.row[0][1], -sin(0.5));
        assert_eq!(t.row[1][0], sin(0.5));
    }

    /// `from_orientation` keeps the translation it was given (`setOrientation` reads it from `m_transform`).
    #[test]
    fn from_orientation_keeps_the_translation() {
        let t = Matrix3D::from_orientation(-1.0, Coord3D::new(7.0, 8.0, 9.0));
        assert_eq!(t.translation(), Coord3D::new(7.0, 8.0, 9.0));
        assert_eq!(t.row[2], [0.0, 0.0, 1.0, 9.0]);
    }
}
